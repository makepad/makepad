//! The shot catalogue. Each generator turns an analysed building into one
//! camera track; [`full_tour`] strings five of them together.
//!
//! Every generator does the same three things and differs only in taste:
//! choose waypoints, choose what each one looks at, choose a motion profile.
//! All the geometry safety lives in [`crate::path::build_track`], which
//! relaxes whatever the generator asked for until it is collision-free
//! against the clearance oracle. A generator cannot produce an unsafe path by
//! being careless; it can only produce a boring one.

use crate::analysis::{ClearMode, SiteAnalysis};
use crate::geom::*;
use crate::path::{build_track, final_gaze, polish, MotionProfile, TrackOpts, Waypoint};
use crate::route::{portal_between, room_order, room_path, route_points, stair_between};
use crate::track::{CameraTrack, ShotKind, TrackNote};
use makepad_math::{vec3, Vec3f};

#[derive(Clone, Copy, Debug)]
pub struct ShotOptions {
    pub fps: f32,
    pub walk: MotionProfile,
    pub drone: MotionProfile,
    pub fov_y_deg: f32,
    /// Cap on rooms in one walkthrough, so a 60-room office does not become a
    /// 20-minute film. `0` = no cap.
    pub max_rooms: usize,
    /// Turns of the reveal spiral. The cheapest way to shorten an exterior
    /// tour: fewer turns rather than a faster camera.
    pub reveal_turns: f32,
    /// Multiplier on every generator's cruise speed, and so the inverse of the
    /// running time. Most shots derive their speed from the building's size,
    /// which is right — a big house wants a faster orbit — but leaves no way to
    /// ask for a particular length of film. This is that dial.
    pub speed_scale: f32,
}

impl Default for ShotOptions {
    fn default() -> Self {
        ShotOptions {
            fps: 30.0,
            walk: MotionProfile::walk(),
            drone: MotionProfile::drone(),
            fov_y_deg: 45.0,
            max_rooms: 14,
            reveal_turns: 1.6,
            speed_scale: 1.0,
        }
    }
}

/// Keep a camera position inside the voxel volume. Outside it there is no
/// clearance information at all, which the oracle correctly reports as zero
/// room — so a shot that wanders out of the grid fails QA for being "inside
/// geometry" when it is really just off the edge of the map.
fn clamp_to_grid(site: &SiteAnalysis, p: Vec3f) -> Vec3f {
    let b = site.grid.bounds();
    let m = site.grid.cell * 6.0;
    vec3(
        p.x.clamp(b.min.x + m, b.max.x - m),
        p.y.clamp(b.min.y + m, b.max.y - m),
        p.z.clamp(b.min.z + m, b.max.z - m),
    )
}

/// The lowest altitude at which a whole ring of radius `rad` around `c` is
/// clear, so an orbit can be flown at a constant, safe height.
///
/// Lifting each waypoint independently produces a spiral that bobs over every
/// bump in the terrain: bad to look at, and the curvature it introduces is
/// worse than the obstacle it avoided. One altitude for the ring is both safer
/// and a better shot.
fn ring_altitude(site: &SiteAnalysis, c: Vec3f, rad: f32, want: f32, from_z: f32) -> f32 {
    let field = site.clearance(ClearMode::Fly);
    let b = site.grid.bounds();
    let top = b.max.z - site.grid.cell * 4.0;
    let step = site.grid.cell * 2.0;
    let mut z = from_z.max(b.min.z + site.grid.cell * 4.0);
    let mut guard = 0;
    while z < top && guard < 600 {
        let ok = (0..24).all(|k| {
            let a = k as f32 * std::f32::consts::TAU / 24.0;
            let p = clamp_to_grid(site, vec3(c.x + a.cos() * rad, c.y + a.sin() * rad, z));
            field.at(p) >= want
        });
        if ok {
            return z;
        }
        z += step;
        guard += 1;
    }
    top
}

/// Raise a camera position until it has room, then clamp it back into the
/// grid. Exterior shots are laid out from the building's bounds, but the
/// ground they fly over is the *site*: on a hillside the orbit radius that
/// clears the house is thirty metres into the slope behind it.
fn lift_clear(site: &SiteAnalysis, p: Vec3f, want: f32) -> Vec3f {
    let field = site.clearance(ClearMode::Fly);
    let top = site.grid.bounds().max.z - site.grid.cell * 4.0;
    let mut q = clamp_to_grid(site, p);
    let step = site.grid.cell * 2.0;
    let mut guard = 0;
    while field.at(q) < want && q.z < top && guard < 400 {
        q.z += step;
        guard += 1;
    }
    clamp_to_grid(site, q)
}

fn plan_radius(site: &SiteAnalysis) -> f32 {
    let s = aabb_size(&site.building);
    (s.x.max(s.y) * 0.5).max(2.0)
}

fn roof_point(site: &SiteAnalysis) -> Vec3f {
    let c = aabb_center(&site.building);
    vec3(c.x, c.y, site.building.max.z)
}

/// Where the approach and the reveal aim. With no door found, aim at the best
/// façade's *outside face* — never at the centre of the plan, which is inside
/// the building and sends the approach shot straight through a wall.
fn entrance_point(site: &SiteAnalysis) -> Vec3f {
    if let Some(e) = &site.entrance {
        return e.center;
    }
    let c = aabb_center(&site.building);
    let r = plan_radius(site);
    let z = (site.building.min.z + site.building.max.z) * 0.5;
    // Aim a little *inside* the façade, so the shots that point here frame the
    // building rather than a point hanging in the air in front of it.
    match site.facades.first() {
        Some(f) => clamp_to_grid(site, vec3(c.x, c.y, z) + f.dir * (r * 0.8)),
        None => vec3(c.x, c.y + r * 0.8, z),
    }
}

/// **(a) Exterior drone reveal.** A rising spiral around the building, radius
/// and altitude keyed to its bounds, gaze drifting from the entrance up to the
/// roofline. The classic establishing shot: it says how big, what shape, and
/// where the front door is, in one move.
pub fn drone_reveal(site: &SiteAnalysis, opt: &ShotOptions) -> CameraTrack {
    let c = aabb_center(&site.building);
    let r = plan_radius(site);
    let base = site.building.min.z;
    let top = site.building.max.z;
    let height = (top - base).max(3.0);

    // Start behind the best façade so the spiral *arrives* at it.
    let start_yaw = site
        .facades
        .first()
        .map(|f| f.dir.y.atan2(f.dir.x) + std::f32::consts::PI * 0.9)
        .unwrap_or(0.0);

    let entrance = entrance_point(site);
    let roof = roof_point(site);
    let prof_clear = opt.drone.clearance + 0.35;
    let turns = opt.reveal_turns.max(0.5);
    let steps = ((turns * 16.0).ceil() as usize).max(12);

    // One safe floor for the widest and the tightest ring, so the spiral only
    // ever rises.
    let rad_out = r * 1.55 + 3.0;
    let rad_in = r * 1.20 + 3.0;
    let floor_out = ring_altitude(site, c, rad_out, prof_clear, base + 1.0);
    let floor_in = ring_altitude(site, c, rad_in, prof_clear, floor_out);
    let alt_lo = floor_out.max(base + height * 0.25) + 1.5;
    let alt_hi = (floor_in.max(top) + (r * 0.45).clamp(3.0, 14.0)).max(alt_lo + 3.0);

    let mut wps = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let f = i as f32 / steps as f32;
        let yaw = start_yaw + f * turns * std::f32::consts::TAU;
        // Close in a little as it rises: a spiral that only rises reads flat.
        let rad = r * (1.55 - 0.35 * f) + 3.0;
        let alt = alt_lo + (alt_hi - alt_lo) * smootherstep(f);
        let pos = clamp_to_grid(site, vec3(c.x + yaw.cos() * rad, c.y + yaw.sin() * rad, alt));
        // Gaze drifts entrance → roofline over the first two thirds.
        let g = smootherstep((f / 0.66).min(1.0));
        let target = Vec3f::from_lerp(entrance, roof, g);
        wps.push(
            Waypoint::new(pos)
                .looking_at(target)
                .fov(opt.fov_y_deg)
                .speed(if f < 0.12 || f > 0.88 { 0.8 } else { 1.0 }),
        );
    }

    let field = site.clearance(ClearMode::Fly);
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof = prof.with_speed(((r * 0.45).clamp(2.0, 7.0)) * opt.speed_scale);
    build_track(
        &field,
        &wps,
        &prof,
        &TrackOpts::new("Exterior reveal", ShotKind::DroneReveal.label()).notes(vec![
            (0, "Establishing".into()),
            (steps, "Roofline".into()),
        ]),
    )
}

/// **(b) Approach.** From far out on the best façade's axis, in to the front
/// door, descending from a slightly raised eye to standing height. A gentle
/// lateral curve keeps it from feeling like a dolly on rails.
pub fn approach(site: &SiteAnalysis, opt: &ShotOptions) -> CameraTrack {
    let target = entrance_point(site);
    let out = match &site.entrance {
        Some(e) => e.outward,
        None => site
            .facades
            .first()
            .map(|f| f.dir)
            .unwrap_or(vec3(1.0, 0.0, 0.0)),
    };
    let r = plan_radius(site);
    let standoff = site
        .facades
        .iter()
        .find(|f| f.dir.dot(out) > 0.7)
        .map(|f| f.standoff)
        .unwrap_or(r * 1.5)
        .max(r * 1.2)
        .clamp(r * 0.9, r * 2.6)
        .min(45.0);

    // Do not start outside the voxel volume: clamping every early waypoint to
    // the same grid edge collapses the approach into a couple of metres.
    let mut standoff = standoff;
    while standoff > 4.0 {
        let start = target + out * standoff;
        if (clamp_to_grid(site, start) - start).length() < 0.2 {
            break;
        }
        standoff *= 0.9;
    }

    let side = Vec3f::cross(out, vec3(0.0, 0.0, 1.0)).normalize();
    let mut wps = Vec::new();
    let steps = 10;
    for i in 0..=steps {
        let f = i as f32 / steps as f32;
        let d = standoff * (1.0 - f) + 2.2 * f;
        // Curve in from one side, straightening as it arrives.
        let lateral = side * (standoff * 0.16 * (1.0 - f) * (1.0 - f));
        let z = target.z + (site.building.max.z - target.z).max(0.0) * 0.35 * (1.0 - f) * (1.0 - f);
        let pos = lift_clear(site, target + out * d + lateral + vec3(0.0, 0.0, z - target.z), opt.drone.clearance + 0.2);
        wps.push(
            Waypoint::new(pos)
                .looking_at(target)
                .fov(opt.fov_y_deg)
                .speed(if f > 0.8 { 0.6 } else { 1.0 }),
        );
    }
    let field = site.clearance(ClearMode::Fly);
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof = prof.with_speed(((standoff / 7.0).clamp(1.6, 4.5)) * opt.speed_scale);
    build_track(
        &field,
        &wps,
        &prof,
        &TrackOpts::new("Approach", ShotKind::Approach.label())
            .notes(vec![(steps, "At the door".into())]),
    )
}

/// A leg of the interior tour: one storey's worth of waypoints.
struct Leg {
    storey: usize,
    wps: Vec<Waypoint>,
    notes: Vec<(usize, String)>,
}

/// Walk the room graph, producing per-storey legs. Shared by the walkthrough
/// and the drone fly-through — they differ in height and profile, not route.
fn tour_legs(site: &SiteAnalysis, opt: &ShotOptions, flying: bool) -> Vec<Leg> {
    let Some(start) = site
        .entrance
        .as_ref()
        .and_then(|e| e.room)
        .or_else(|| site.rooms_by_rank().first().copied())
    else {
        return Vec::new();
    };
    // `room_order` is a walk, so it repeats rooms it passes back through.
    // Cap on *distinct* rooms, and cut at that point rather than at a raw
    // index — truncating mid-backtrack would leave the last two entries
    // non-adjacent, which is the thing the walk exists to prevent.
    // Tour from the front door — unless the front door's own region is a
    // dead end. A building can be split (a bricked-up doorway, a wing reached
    // only from outside), and a tour that starts at the entrance and finds one
    // room there should show the rest of the house rather than stop.
    let mut start = start;
    let mut order = room_order(site, start);
    let interior_total = site.rooms.iter().filter(|r| r.interior).count();
    if order.len() * 2 < interior_total {
        let entrance_storey = site
            .entrance
            .as_ref()
            .and_then(|e| e.room)
            .map(|r| site.rooms[r].storey);
        for cand in site.rooms_by_rank() {
            if entrance_storey.is_some_and(|s| site.rooms[cand].storey != s) {
                continue;
            }
            let alt = room_order(site, cand);
            if alt.len() > order.len() {
                order = alt;
                start = cand;
            }
        }
    }
    if opt.max_rooms > 0 {
        let mut seen: Vec<usize> = Vec::new();
        let mut cut = order.len();
        for (i, r) in order.iter().enumerate() {
            if !seen.contains(r) {
                seen.push(*r);
                if seen.len() > opt.max_rooms {
                    cut = i;
                    break;
                }
            }
        }
        order.truncate(cut.max(1));
    }
    // Come back to where we came in. Besides being what a tour does, it is
    // what lets the exit leg start from the entrance's own storey: leaving the
    // camera upstairs and then routing to the front door on the ground-floor
    // lattice draws a line straight down through the slab.
    if let Some(&last) = order.last() {
        if last != start {
            let back = room_path(site, last, start);
            order.extend(back.into_iter().skip(1));
        }
    }
    let mut noted: Vec<usize> = Vec::new();

    let mut legs: Vec<Leg> = Vec::new();
    let mut cur_storey = site.rooms[start].storey;
    let mut leg = Leg {
        storey: cur_storey,
        wps: Vec::new(),
        notes: Vec::new(),
    };
    let mut cursor: Option<Vec3f> = None;
    if order.len() == 1 && !flying {
        if let Some(e) = site.entrance.as_ref().filter(|e| e.room == Some(order[0])) {
            let inside = e.center - e.outward * 1.2;
            leg.wps.push(Waypoint::new(inside).speed(0.8).fov(opt.fov_y_deg));
            cursor = Some(inside);
        }
    }

    // Altitude for the fly-through breathes along the *path*, not per room:
    // one height per room means a vertical step at every doorway, and a step
    // in a spline is a corner the camera has to whip through.
    let mut travelled = 0.0f32;

    for (oi, &room) in order.iter().enumerate() {
        let r = &site.rooms[room];
        if r.storey != cur_storey {
            // A change of storey means a staircase.
            let prev = order[oi - 1];
            if let Some(stair) = stair_between(site, prev, room) {
                let up = site.rooms[prev].storey < r.storey;
                let (from, to) = if up {
                    (stair.bottom, stair.top)
                } else {
                    (stair.top, stair.bottom)
                };
                let (ramp_a, ramp_b) = if up {
                    (stair.run_low, stair.run_high)
                } else {
                    (stair.run_high, stair.run_low)
                };
                if let Some(c) = cursor {
                    if let Some(pts) = route_points(site, cur_storey, c, from) {
                        push_route(&mut leg, &pts, cur_storey, site, flying, &mut travelled);
                    }
                }
                leg.wps.push(Waypoint::new(from).speed(0.7).fov(opt.fov_y_deg));
                legs.push(std::mem::replace(
                    &mut leg,
                    Leg {
                        storey: r.storey,
                        wps: Vec::new(),
                        notes: Vec::new(),
                    },
                ));
                // The stair transit is its own short leg, planned in the air:
                // a staircase belongs to neither floor's plan lattice.
                // Enough intermediate points that the relaxer can lift the
                // line clear of the treads below and the slab edge above. A
                // single midpoint gives it nothing to bend.
                let mut swps = vec![Waypoint::new(from).speed(0.75).fov(opt.fov_y_deg)];
                let steps = 8;
                for k in 0..=steps {
                    let f = k as f32 / steps as f32;
                    let mut p = Vec3f::from_lerp(ramp_a, ramp_b, f);
                    // Climb with headroom rather than cutting the diagonal.
                    p.z += (f * std::f32::consts::PI).sin() * 0.20;
                    swps.push(Waypoint::new(p).speed(0.75).fov(opt.fov_y_deg));
                }
                swps.push(Waypoint::new(to).speed(0.75).fov(opt.fov_y_deg));
                legs.push(Leg {
                    storey: usize::MAX,
                    wps: swps,
                    notes: vec![(0, format!("Stairs to {}", site.storeys[r.storey].name))],
                });
                cursor = Some(to);
            }
            cur_storey = r.storey;
            leg.storey = cur_storey;
        }

        let target = if flying {
            vec3(r.center.x, r.center.y, fly_height(site, r.storey, travelled))
        } else {
            r.center
        };

        if let Some(c) = cursor {
            // Pin the doorway so the path goes through the middle of it.
            let via = (oi > 0)
                .then(|| portal_between(site, order[oi - 1], room).map(|p| p.center))
                .flatten();
            let mut ok = false;
            if let Some(v) = via {
                if let (Some(a), Some(b)) = (
                    route_points(site, cur_storey, c, v),
                    route_points(site, cur_storey, v, target),
                ) {
                    push_route(&mut leg, &a, cur_storey, site, flying, &mut travelled);
                    let n = leg.wps.len();
                    if n > 0 {
                        leg.wps[n - 1] = leg.wps[n - 1].pin().speed(0.75);
                    }
                    push_route(&mut leg, &b[1..], cur_storey, site, flying, &mut travelled);
                    ok = true;
                }
            }
            if !ok {
                if let Some(pts) = route_points(site, cur_storey, c, target) {
                    push_route(&mut leg, &pts, cur_storey, site, flying, &mut travelled);
                }
            }
        }

        // Arrive, slow down, and turn to the best thing in the room.
        let look = if flying {
            vec3(r.best_view.x, r.best_view.y, target.z + (r.best_view.z - r.center.z))
        } else {
            r.best_view
        };
        let first_time = !noted.contains(&room);
        if first_time {
            noted.push(room);
            leg.notes
                .push((leg.wps.len(), format!("{} · {:.0} m²", r.name, r.area)));
        }
        leg.wps.push(
            Waypoint::new(target)
                .looking_at(look)
                // Linger on arrival, but pass straight through a room being
                // re-tread on the way somewhere else.
                .speed(if first_time { 0.5 } else { 0.9 })
                .fov(opt.fov_y_deg),
        );
        cursor = Some(target);
    }
    if !leg.wps.is_empty() {
        legs.push(leg);
    }
    legs.retain(|l| l.wps.len() >= 2);
    legs
}

/// Fly-through altitude at `travelled` metres along the route. One slow sine
/// over distance, clamped into the storey's headroom.
fn fly_height(site: &SiteAnalysis, storey: usize, travelled: f32) -> f32 {
    let st = &site.storeys[storey];
    // Cap below a standard 2.05 m door head: the fly-through goes through
    // doorways, and a drone at 1.9 m has 0.15 m of hair between it and the
    // lintel.
    let head = (st.height - 0.7).min(1.70).max(1.2);
    st.floor_z + (1.35 + 0.35 * (travelled * 0.16).sin()).clamp(1.0, head)
}

fn push_route(
    leg: &mut Leg,
    pts: &[Vec3f],
    storey: usize,
    site: &SiteAnalysis,
    flying: bool,
    travelled: &mut f32,
) {
    let st = &site.storeys[storey];
    let mut prev: Option<Vec3f> = leg.wps.last().map(|w| w.pos);
    for p in pts {
        if let Some(q) = prev {
            *travelled += ((p.x - q.x).powi(2) + (p.y - q.y).powi(2)).sqrt();
        }
        prev = Some(*p);
        let z = if flying {
            fly_height(site, storey, *travelled)
        } else {
            st.eye_z
        };
        leg.wps.push(Waypoint::new(vec3(p.x, p.y, z)));
    }
}

fn build_legs(
    site: &SiteAnalysis,
    legs: &[Leg],
    profile: &MotionProfile,
    name: &str,
    kind: ShotKind,
    flying: bool,
) -> CameraTrack {
    let mut out = CameraTrack {
        name: name.into(),
        kind_label: kind.label().into(),
        keys: Vec::new(),
        fps: profile.fps,
        notes: Vec::new(),
    };
    // The legs are one continuous move, not a sequence of shots: ease only at
    // the two real ends, and hand each leg the gaze the last one finished on.
    let mut gaze: Option<(f32, f32)> = None;
    for (i, leg) in legs.iter().enumerate() {
        let mode = if flying || leg.storey == usize::MAX {
            ClearMode::Fly
        } else {
            ClearMode::Walk(leg.storey)
        };
        let field = site.clearance(mode);
        let mut prof = *profile;
        if leg.storey == usize::MAX {
            // Stairs are tight by nature; ask for less and let QA judge.
            prof.clearance = prof.clearance.max(0.30);
            prof.relax_margin = 0.02;
            prof.speed *= 0.7;
        }
        let mut o = TrackOpts::new(name, kind.label()).notes(leg.notes.clone());
        o.ease_in = i == 0;
        o.ease_out = i + 1 == legs.len();
        o.initial_gaze = gaze;
        let t = build_track(&field, &leg.wps, &prof, &o);
        gaze = final_gaze(&t).or(gaze);
        out.append(&t, 0.0);
    }
    // The legs are individually smooth and meet at corners. Polish the joined
    // path once: it rounds the joins against the clearance oracle and re-times
    // the whole run, keeping each leg's own pacing.
    let mut pp = *profile;
    pp.clearance = 0.25;
    let fld = site.clearance(ClearMode::Fly);
    polish(&fld, &out, &pp)
}

/// **(c) Interior walkthrough.** On foot at eye height, room to room through
/// the door graph in POI order, centred in every doorway, slowing in each room
/// and turning to its best view.
pub fn walkthrough(site: &SiteAnalysis, opt: &ShotOptions) -> CameraTrack {
    let legs = tour_legs(site, opt, false);
    let mut prof = opt.walk;
    prof.fps = opt.fps;
    build_legs(site, &legs, &prof, "Walkthrough", ShotKind::Walkthrough, false)
}

/// **(d) Drone fly-through.** The same route, flown: higher, faster, with the
/// altitude breathing through the storey's headroom.
pub fn drone_flythrough(site: &SiteAnalysis, opt: &ShotOptions) -> CameraTrack {
    let legs = tour_legs(site, opt, true);
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof = prof.with_speed((opt.drone.speed * 0.55) * opt.speed_scale);
    prof.clearance = 0.30;
    // Indoors the turns are tight; a gaze that cannot keep up holds walls.
    prof.max_gaze_rate = 1.05;
    build_legs(
        site,
        &legs,
        &prof,
        "Interior fly-through",
        ShotKind::DroneFlythrough,
        true,
    )
}

/// What an orbit goes around.
#[derive(Clone, Copy, Debug)]
pub enum OrbitTarget {
    Room(usize),
    Bounds(makepad_math::Aabb),
}

/// **(e) Orbit.** One turn around a room or an element, at a radius and height
/// that frame it.
pub fn orbit(site: &SiteAnalysis, target: OrbitTarget, opt: &ShotOptions) -> CameraTrack {
    let (bounds, name) = match target {
        OrbitTarget::Room(r) => (site.rooms[r].bounds, site.rooms[r].name.clone()),
        OrbitTarget::Bounds(b) => (b, "Selection".to_string()),
    };
    let c = aabb_center(&bounds);
    let s = aabb_size(&bounds);
    let alt = c.z + s.z * 0.35;
    let steps = 32;
    let field = site.clearance(ClearMode::Fly);

    // An interior room is smaller than the orbit it "deserves": a ring sized
    // from the room's bounds puts the camera in the walls. Shrink until the
    // whole ring has air, and give up on the shot rather than fly through a
    // wall to take it.
    let want = opt.drone.clearance;
    let ring_ok = |rad: f32| -> bool {
        (0..24).all(|i| {
            let yaw = i as f32 * std::f32::consts::TAU / 24.0;
            field.at(vec3(c.x + yaw.cos() * rad, c.y + yaw.sin() * rad, alt)) >= want
        })
    };
    let ideal = (s.x.max(s.y) * 0.5).max(1.0) * 1.9 + want * 2.0;
    let mut rad = ideal;
    while rad > 0.8 && !ring_ok(rad) {
        rad *= 0.9;
    }
    if !ring_ok(rad) {
        return CameraTrack::default();
    }

    let mut wps = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let f = i as f32 / steps as f32;
        let yaw = f * std::f32::consts::TAU;
        wps.push(
            Waypoint::new(vec3(c.x + yaw.cos() * rad, c.y + yaw.sin() * rad, alt))
                .looking_at(c)
                .fov(opt.fov_y_deg),
        );
    }
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof = prof.with_speed(((rad * 0.5).clamp(1.2, 4.0)) * opt.speed_scale);
    build_track(
        &field,
        &wps,
        &prof,
        &TrackOpts::new(&format!("Orbit · {name}"), ShotKind::Orbit.label())
            .notes(vec![(0, format!("Orbit {name}"))]),
    )
}

/// **(f) Storey reveal.** One slow pass per storey at that level's height,
/// looking inward and down. The viewer is expected to section the model above
/// the storey while the pass runs; because [`CameraTrack`] carries no section
/// state, each pass emits a note of the form `section_z=<metres> · <storey>`
/// for the app to act on. That keeps this crate free of viewer state and still
/// makes the shot real rather than a stub.
pub fn storey_reveal(site: &SiteAnalysis, opt: &ShotOptions) -> CameraTrack {
    let c = aabb_center(&site.building);
    let r = plan_radius(site);
    let field = site.clearance(ClearMode::Fly);
    let mut out = CameraTrack {
        name: "Storey reveal".into(),
        kind_label: ShotKind::StoreyReveal.label().into(),
        keys: Vec::new(),
        fps: opt.fps,
        notes: Vec::new(),
    };
    let mut gaze = None;
    let mut storeys: Vec<usize> = (0..site.storeys.len()).collect();
    storeys.sort_by(|a, b| {
        site.storeys[*a]
            .elevation
            .partial_cmp(&site.storeys[*b].elevation)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (k, si) in storeys.iter().enumerate() {
        let st = &site.storeys[*si];
        let alt = st.floor_z + (r * 0.55).max(4.0);
        let steps = 12;
        let mut wps = Vec::with_capacity(steps + 1);
        let yaw0 = k as f32 * 0.7;
        for i in 0..=steps {
            let f = i as f32 / steps as f32;
            let yaw = yaw0 + f * std::f32::consts::PI * 0.75;
            let rad = r * 1.5 + 4.0;
            wps.push(
                Waypoint::new(lift_clear(
                    site,
                    vec3(c.x + yaw.cos() * rad, c.y + yaw.sin() * rad, alt),
                    opt.drone.clearance + 0.35,
                ))
                .looking_at(vec3(c.x, c.y, st.floor_z + 1.0))
                    .fov(opt.fov_y_deg),
            );
        }
        let mut prof = opt.drone;
        prof.fps = opt.fps;
        prof = prof.with_speed(((r * 0.35).clamp(1.5, 5.0)) * opt.speed_scale);
        let mut o = TrackOpts::new("Storey reveal", ShotKind::StoreyReveal.label()).notes(vec![(
            0,
            format!("section_z={:.3} · {}", st.floor_z + st.height, st.name),
        )]);
        o.ease_in = k == 0;
        o.ease_out = k + 1 == storeys.len();
        o.initial_gaze = gaze;
        let t = build_track(&field, &wps, &prof, &o);
        if let (Some(last), Some(first)) = (out.keys.last().copied(), t.keys.first()) {
            if (first.pos - last.pos).length() > 0.6 {
                let tr = transit(site, last.pos, first.pos, gaze, opt);
                if tr.keys.len() >= 2 {
                    out.append(&tr, 0.0);
                }
            }
        }
        gaze = final_gaze(&t).or(gaze);
        out.append(&t, 0.0);
    }
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof.clearance = 0.40;
    // Joined by transits, so the corners are at the seams: hold the limit
    // well under what QA allows rather than at it.
    prof.max_lateral_accel = 1.6;
    polish(&field, &out, &prof)
}

/// The exit leg: from wherever the walkthrough finished, back to the entrance
/// and out through it.
fn exit_leg(
    site: &SiteAnalysis,
    opt: &ShotOptions,
    from: Vec3f,
    gaze: Option<(f32, f32)>,
) -> CameraTrack {
    let Some(e) = &site.entrance else {
        return CameraTrack::default();
    };
    let Some(room) = e.room else {
        return CameraTrack::default();
    };
    let si = site.rooms[room].storey;
    // Only valid if we are already on the entrance's storey; otherwise there
    // is a staircase between here and the door and this is not the shot to
    // take it.
    if (from.z - site.storeys[si].eye_z).abs() > 1.0 {
        return CameraTrack::default();
    }
    let inside = e.center - e.outward * 1.4;
    let Some(pts) = route_points(site, si, from, inside) else {
        return CameraTrack::default();
    };
    let mut wps: Vec<Waypoint> = pts.iter().map(|p| Waypoint::new(*p)).collect();
    // Through the door, then a few metres clear of it.
    wps.push(Waypoint::new(e.center).pin().speed(0.7));
    wps.push(Waypoint::new(e.center + e.outward * 3.0).speed(0.9));
    let field = site.clearance(ClearMode::Fly);
    let mut prof = opt.walk;
    prof.fps = opt.fps;
    prof.clearance = 0.30;
    let mut o = TrackOpts::new("Exit", ShotKind::Walkthrough.label())
        .notes(vec![(wps.len().saturating_sub(2), "Out the front door".into())]);
    o.initial_gaze = gaze;
    build_track(&field, &wps, &prof, &o)
}

/// The closing pull-back: retreat from the entrance while rising, ending on
/// the whole building.
fn pullback(site: &SiteAnalysis, opt: &ShotOptions, gaze: Option<(f32, f32)>) -> CameraTrack {
    let e = entrance_point(site);
    let out = site
        .entrance
        .as_ref()
        .map(|x| x.outward)
        .unwrap_or(vec3(1.0, 0.0, 0.0));
    let r = plan_radius(site);
    let roof = roof_point(site);
    let steps = 12;
    let mut wps = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        let f = i as f32 / steps as f32;
        let d = 3.0 + (r * 2.4) * smootherstep(f);
        let z = e.z + (roof.z - e.z + r * 0.5) * smootherstep(f);
        let bc = aabb_center(&site.building);
        let target = Vec3f::from_lerp(e, vec3(bc.x, bc.y, bc.z), smootherstep(f));
        wps.push(
            Waypoint::new(lift_clear(site, e + out * d + vec3(0.0, 0.0, z - e.z), opt.drone.clearance + 0.35))
                .looking_at(target)
                .fov(opt.fov_y_deg)
                .speed(if f < 0.15 { 0.7 } else { 1.0 }),
        );
    }
    let field = site.clearance(ClearMode::Fly);
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof = prof.with_speed(((r * 0.4).clamp(2.0, 6.0)) * opt.speed_scale);
    let mut o = TrackOpts::new("Pull-back", ShotKind::DroneReveal.label())
        .notes(vec![(steps, "Final frame".into())]);
    o.initial_gaze = gaze;
    build_track(&field, &wps, &prof, &o)
}

/// A connecting move between two shots: fly from where one ended to where the
/// next begins, bulging upward so the link clears whatever is between them.
///
/// Without this the full tour would cut — and a cut is a teleport as far as
/// the QA is concerned, which is right: this crate promises *one continuous
/// camera*, so the joins have to be flown, not spliced.
fn transit(
    site: &SiteAnalysis,
    from: Vec3f,
    to: Vec3f,
    gaze: Option<(f32, f32)>,
    opt: &ShotOptions,
) -> CameraTrack {
    let d = to - from;
    let len = d.length();
    if len < 0.6 {
        return CameraTrack::default();
    }
    let top = site.building.max.z;

    // Does this link cross the building envelope? If so it has to use the
    // door. A straight flight from a room to a point out on the lawn is the
    // shortest path and also, invariably, straight through the façade.
    let indoors = |p: Vec3f| {
        site.grid
            .cell_of(p)
            .map_or(false, |(x, y, z)| !site.grid.exterior_at(x, y, z))
    };
    let crossing = indoors(from) != indoors(to);

    let mut wps = vec![Waypoint::new(from).fov(opt.fov_y_deg)];
    if crossing {
        let Some(e) = &site.entrance else {
            // Nowhere known to cross. Better to admit the tour cannot get
            // through than to invent a hole in the wall.
            return CameraTrack::default();
        };
        let field = site.clearance(ClearMode::Fly);
        let side = Vec3f::cross(e.outward, vec3(0.0, 0.0, 1.0));
        let side = if side.length_squared() < 1e-6 {
            vec3(0.0, 1.0, 0.0)
        } else {
            side.normalize()
        };
        // Don't pin the wall plane (clearance 0). Slide sideways until the
        // chord through the opening is actually free.
        let mut seq = None;
        for off in [0.0f32, 0.25, -0.25, 0.5, -0.5, 0.75, -0.75] {
            let inner = e.center - e.outward * 1.6 + side * off;
            let outer = e.center + e.outward * 2.4 + side * off;
            let gap = e.center - e.outward * 0.25 + side * off;
            if field.at(gap) >= 0.12 && field.segment_clear(outer, inner, 0.10) {
                seq = Some(if indoors(from) {
                    [inner, gap, outer]
                } else {
                    [outer, gap, inner]
                });
                break;
            }
        }
        let Some(seq) = seq else {
            return CameraTrack::default();
        };
        for p in seq {
            wps.push(Waypoint::new(p).speed(0.8).fov(opt.fov_y_deg));
        }
    } else {
        let lift = (len * 0.18).clamp(0.0, 6.0);
        let mid = Vec3f::from_lerp(from, to, 0.5) + vec3(0.0, 0.0, lift);
        // Stay inside the voxel volume: clearance outside it reads as zero,
        // and a transit that leaves the grid is one the QA calls a wall.
        let ceiling = (site.grid.bounds().max.z - 1.0).max(top);
        let mid =
            clamp_to_grid(site, vec3(mid.x, mid.y, mid.z.max(from.z.min(to.z)).min(ceiling)));
        wps.push(Waypoint::new(Vec3f::from_lerp(from, mid, 0.6)).fov(opt.fov_y_deg));
        wps.push(Waypoint::new(mid).fov(opt.fov_y_deg));
        wps.push(Waypoint::new(Vec3f::from_lerp(mid, to, 0.4)).fov(opt.fov_y_deg));
    }
    wps.push(Waypoint::new(to).fov(opt.fov_y_deg));
    let field = site.clearance(ClearMode::Fly);
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof = prof.with_speed(((len / 4.0).clamp(1.5, 6.0)) * opt.speed_scale);
    prof.clearance = 0.35;
    let mut o = TrackOpts::new("Transit", ShotKind::FullTour.label());
    o.ease_in = false;
    o.ease_out = false;
    o.initial_gaze = gaze;
    let t = build_track(&field, &wps, &prof, &o);

    // Verify before promising. A transit is the one leg built from guesses
    // rather than from a route, so check it actually flies: if the relaxer
    // could not pull it clear, report no link and let the caller drop the leg
    // instead of splicing a camera through a façade.
    if t.keys.len() < 2 {
        return CameraTrack::default();
    }
    let worst = t
        .keys
        .iter()
        .map(|k| field.at(k.pos))
        .fold(f32::INFINITY, f32::min);
    if worst < 0.15 {
        return CameraTrack::default();
    }
    t
}

/// **The full tour**: reveal → approach → walkthrough → exit → pull-back, flown
/// as one continuous camera with transits stitching the legs together.
pub fn full_tour(site: &SiteAnalysis, opt: &ShotOptions) -> CameraTrack {
    let mut out = CameraTrack {
        name: "Full tour".into(),
        kind_label: ShotKind::FullTour.label().into(),
        keys: Vec::new(),
        fps: opt.fps,
        notes: Vec::new(),
    };
    let mut gaze: Option<(f32, f32)> = None;

    let push = |out: &mut CameraTrack, leg: &CameraTrack, label: Option<&str>, gaze: &mut Option<(f32, f32)>| {
        if leg.keys.len() < 2 {
            return;
        }
        if let (Some(last), Some(first)) = (out.keys.last().copied(), leg.keys.first()) {
            // Stitch: never let two legs be spliced with a gap in space.
            if (first.pos - last.pos).length() > 0.6 {
                let t = transit(site, last.pos, first.pos, *gaze, opt);
                if t.keys.len() < 2 {
                    // No flyable link exists — most often because the tour is
                    // indoors and the building has no findable door. Drop the
                    // leg rather than splice it on: a splice is a teleport, and
                    // a teleport across a façade is a camera in a wall.
                    if std::env::var("TOUR_STITCH").is_ok() {
                        eprintln!(
                            "      [stitch] drop '{}' gap {:.2} m  from {:?} to {:?}",
                            leg.name,
                            (first.pos - last.pos).length(),
                            last.pos,
                            first.pos
                        );
                    }
                    return;
                }
                out.append(&t, 0.0);
                *gaze = final_gaze(&t).or(*gaze);
            }
        }
        if let Some(l) = label {
            out.notes.push(TrackNote {
                t: out.duration(),
                text: l.to_string(),
            });
        }
        out.append(leg, 0.0);
        *gaze = final_gaze(leg).or(*gaze);
    };

    let reveal = drone_reveal(site, opt);
    push(&mut out, &reveal, Some("Exterior reveal"), &mut gaze);
    let app = approach(site, opt);
    push(&mut out, &app, Some("Approach"), &mut gaze);
    let walk = walkthrough(site, opt);
    push(&mut out, &walk, Some("Walkthrough"), &mut gaze);

    if let Some(p) = out.keys.last().map(|k| k.pos) {
        let ex = exit_leg(site, opt, p, gaze);
        push(&mut out, &ex, Some("Exit"), &mut gaze);
    }
    // Only pull back if the tour actually made it outdoors. A building with
    // no findable way out ends its tour indoors, which is honest; cutting to
    // an exterior shot would be a teleport through the wall.
    let outdoors = out.keys.last().map_or(false, |k| {
        site.grid
            .cell_of(k.pos)
            .map_or(false, |(x, y, z)| site.grid.exterior_at(x, y, z))
    });
    if outdoors || site.entrance.is_some() {
        let pb = pullback(site, opt, gaze);
        push(&mut out, &pb, Some("Pull-back"), &mut gaze);
    }

    // Five shots concatenated are geometrically right and temporally wrong:
    // each eased its own ends and started its own gaze limiter. Re-time the
    // whole thing once so it reads as one continuous camera.
    let mut prof = opt.drone;
    prof.fps = opt.fps;
    prof = prof.with_speed((opt.drone.speed * 0.8) * opt.speed_scale);
    prof.ease = 2.0;
    prof.max_lateral_accel = 1.45;
    // The tour goes indoors, so the relax target has to be something a
    // corridor can actually offer.
    prof.clearance = 0.30;
    let field = site.clearance(ClearMode::Fly);
    polish(&field, &out, &prof)
}

/// Everything worth offering in the Tours panel for this building.
pub fn all_shots(site: &SiteAnalysis, opt: &ShotOptions) -> Vec<CameraTrack> {
    let mut v = vec![
        drone_reveal(site, opt),
        approach(site, opt),
        walkthrough(site, opt),
        drone_flythrough(site, opt),
        storey_reveal(site, opt),
        full_tour(site, opt),
    ];
    if let Some(best) = site.rooms_by_rank().first().copied() {
        v.push(orbit(site, OrbitTarget::Room(best), opt));
    }
    v.retain(|t| t.keys.len() >= 2);
    v
}
