//! A light's aim (B9): which way its face points, turned off the direction
//! to the centre, and what hangs on the face: a honeycomb grid and four barn
//! doors.
//!
//! Aim does not change the outline the map shows: a light keeps its angular
//! size and shape, and a diffuse face is as bright from the side as from the
//! front. What changes is how much of the face the centre can see, which is
//! where an environment map is seen from. So an aimed light only looks
//! different through its grid, past its doors, or when it is nearly edge-on.
//!
//! Positions are face coordinates (x to the viewer's right, y up, tangent
//! units); the aimed face's own axes are `r` (its right), `u` (its up) and
//! `n` (its normal, the way it shines).

use super::*;

/// Honeycomb cells across the light's smaller side.
pub const GRID_CELLS: f32 = 12.0;
/// Cell depth over cell width at grid 1. A tube shows nothing once the ray
/// is displaced by a whole cell width over its depth, which is at
/// atan(width / depth) off its axis: 1 / tan(15 degrees), a 15 degree grid.
pub const GRID_DEPTH_MAX: f32 = 3.732;
/// Half the thickness of a cell wall at full strength, and the softening of
/// a cell's edge, both in cell widths.
const GRID_WALL: f32 = 0.04;
const GRID_EDGE: f32 = 0.04;
const SQRT_3: f32 = 1.732_050_8;

#[derive(Clone, Debug)]
pub struct Aim {
    /// The aimed face's normal (the way it shines) and its right and up axes.
    n: Vec3f,
    r: Vec3f,
    u: Vec3f,
    half: Vec2f,
    /// A honeycomb cell's width (flat side to flat side), tangent units.
    cell: f32,
    /// Cell depth over width; 0 without a grid.
    grid_depth: f32,
    /// Half a cell wall's thickness, in cell widths.
    grid_wall: f32,
    /// Each door's flap angle from lying flat outward, radians: top, right, bottom, left.
    flaps: [f32; 4],
    /// The soft edge of a door's shadow on each axis, tangent units.
    soft: Vec2f,
}

impl Aim {
    /// The aim of a light whose face looks back along `center` (the unit
    /// direction to the light), with `right` the viewer's right and `up`
    /// spanning the face, tangent half extents `half`, and edge `softness`
    /// 0..=1. None when the aim is neutral, so a plain light pays nothing.
    pub fn new(a: &AimParams, center: Vec3f, right: Vec3f, up: Vec3f, half: Vec2f, softness: f32) -> Option<Aim> {
        // Every default is 0, so NaN reads as 0.
        let fix = |x: f32, lo: f32, hi: f32| if x.is_nan() { 0.0 } else { x.max(lo).min(hi) };
        let yaw = fix(a.yaw_deg, -90.0, 90.0).to_radians();
        let pitch = fix(a.pitch_deg, -90.0, 90.0).to_radians();
        let grid = fix(a.grid, 0.0, 1.0);
        let doors = a.barn_doors.map(|d| fix(d, 0.0, 90.0));
        if yaw == 0.0 && pitch == 0.0 && grid == 0.0 && doors.iter().all(|d| *d == 0.0) {
            return None;
        }
        // Facing the centre, then yawed about the face's up axis toward the
        // viewer's right, then pitched upward about its (yawed) right axis.
        let n0 = -center;
        let n1 = n0 * yaw.cos() + right * yaw.sin();
        let r = right * yaw.cos() - n0 * yaw.sin();
        let n = n1 * pitch.cos() + up * pitch.sin();
        let u = up * pitch.cos() - n1 * pitch.sin();
        let half = vec2f(half.x.max(1.0e-4), half.y.max(1.0e-4));
        let edge = 0.01 + 0.05 * fix(softness, 0.0, 1.0);
        Some(Aim {
            n,
            r,
            u,
            half,
            cell: 2.0 * half.x.min(half.y) / GRID_CELLS,
            grid_depth: grid * GRID_DEPTH_MAX,
            // The walls thin out with the grid, so grid 0.01 is not a jump from none.
            grid_wall: GRID_WALL * (4.0 * grid).min(1.0),
            flaps: doors.map(|d| (2.0 * d).to_radians()),
            soft: vec2f(edge * half.x, edge * half.y),
        })
    }

    /// A honeycomb hangs on the face.
    pub fn has_grid(&self) -> bool {
        self.grid_depth > 0.0
    }

    /// The fraction 0..=1 of the face at face coordinates `q` (`dir` is the
    /// direction through it) that the centre sees past the doors and through
    /// the grid.
    pub fn transmission(&self, dir: Vec3f, q: Vec2f) -> f32 {
        // The ray from the face to the centre, in the face's own axes.
        let v = -dir;
        let vz = v.dot(self.n);
        if !(vz > 0.0) {
            return 0.0;
        }
        // Its slopes toward the face's right and up, per unit along its normal.
        let tx = (v.dot(self.r) / vz).max(-50.0).min(50.0);
        let ty = (v.dot(self.u) / vz).max(-50.0).min(50.0);
        // A face seen nearly edge-on fades out rather than cutting off.
        let mut t = smoothstep(0.0, 0.08, vz);
        t *= door(q.y, self.half.y, self.flaps[0], ty, self.soft.y);
        t *= door(q.x, self.half.x, self.flaps[1], tx, self.soft.x);
        t *= door(-q.y, self.half.y, self.flaps[2], -ty, self.soft.y);
        t *= door(-q.x, self.half.x, self.flaps[3], -tx, self.soft.x);
        if self.grid_depth > 0.0 {
            t *= self.grid(q, tx, ty);
        }
        t
    }

    /// The honeycomb: hexagonal tubes on the face, `grid_depth` cell widths
    /// deep. The ray that leaves the face at `q` with slopes (tx, ty) comes
    /// out of a tube's mouth displaced by depth x slope; it gets out only if
    /// that point is still inside the cell it started in. Both ends of the
    /// ray are measured against that one cell's walls, so the cell pattern
    /// and the falloff with the angle are the same computation, and a ray
    /// along the axis sees the whole cell less its walls.
    fn grid(&self, q: Vec2f, tx: f32, ty: f32) -> f32 {
        let at = hex_cell(q / self.cell);
        let out = vec2f(at.x + tx * self.grid_depth, at.y + ty * self.grid_depth);
        let worst = hex_dist(at).max(hex_dist(out));
        let open = 0.5 - self.grid_wall;
        1.0 - smoothstep(open - GRID_EDGE, open + GRID_EDGE, worst)
    }
}

/// The offset of `p` from the centre of its honeycomb cell. Cells are regular
/// hexagons one unit across between their flat sides, which face left and
/// right; rows are sqrt(3)/2 apart and every other row is shifted by half a
/// unit. The lattice is two interleaved rectangular grids of 1 x sqrt(3):
/// the nearer of the two candidate centres is the cell's.
fn hex_cell(p: Vec2f) -> Vec2f {
    let a = vec2f(p.x - p.x.floor() - 0.5, p.y - SQRT_3 * (p.y / SQRT_3).floor() - 0.5 * SQRT_3);
    let (sx, sy) = (p.x - 0.5, p.y - 0.5 * SQRT_3);
    let b = vec2f(sx - sx.floor() - 0.5, sy - SQRT_3 * (sy / SQRT_3).floor() - 0.5 * SQRT_3);
    if a.x * a.x + a.y * a.y < b.x * b.x + b.y * b.y {
        a
    } else {
        b
    }
}

/// How far the offset `v` from a cell's centre is toward that cell's edge:
/// 0 at the centre, 0.5 on the hexagon.
fn hex_dist(v: Vec2f) -> f32 {
    let (x, y) = (v.x.abs(), v.y.abs());
    x.max(0.5 * x + 0.5 * SQRT_3 * y)
}

/// How much of the face shows at coordinate `y0` along one axis (half size
/// `h`), past the flap hinged on that axis's + edge. The flap is as long as
/// the half size (two opposite doors closed meet in the middle). It is folded
/// `flap` radians from lying flat outward (0), through straight out (π/2), to
/// closed across the face (π). `slope` is the slope, toward that edge, of
/// the ray from the face to the centre. The flap's tip sits at
/// (h + h·cos flap) along the axis and (h·sin flap) out from the face; the
/// ray crosses the flap, and is blocked, exactly when it starts on the hinge
/// side of the point from which it would graze the tip:
/// y0 > h + h·(cos flap − sin flap · slope).
fn door(y0: f32, h: f32, flap: f32, slope: f32, soft: f32) -> f32 {
    if !(flap > 0.0) {
        return 1.0;
    }
    let edge = h + h * (flap.cos() - flap.sin() * slope);
    1.0 - smoothstep(edge - soft, edge + soft, y0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::light_fx::face_axes;

    /// An aimed square light `size_deg` across, due east on the horizon,
    /// with the axes its face coordinates run along.
    struct Face {
        aim: Aim,
        center: Vec3f,
        right: Vec3f,
        up: Vec3f,
        half: Vec2f,
    }

    impl Face {
        fn new(a: AimParams, size_deg: f32) -> Face {
            let light = LightParams { azimuth_deg: 90.0, elevation_deg: 0.0, width_deg: size_deg, height_deg: size_deg, ..LightParams::default() };
            let (frame, sx, half) = face_axes(&light);
            let right = frame.right * sx;
            let aim = Aim::new(&a, frame.center, right, frame.up, half, 0.2).expect("not neutral");
            Face { aim, center: frame.center, right, up: frame.up, half }
        }

        /// What shows at face coordinates (x, y), given as fractions of the half size.
        fn at(&self, x: f32, y: f32) -> f32 {
            let q = vec2f(x * self.half.x, y * self.half.y);
            let dir = (self.center + self.right * q.x + self.up * q.y).normalize();
            self.aim.transmission(dir, q)
        }

        fn mean(&self) -> f32 {
            let n = 120;
            let mut sum = 0.0f64;
            for j in 0..n {
                for i in 0..n {
                    sum += self.at((i as f32 + 0.5) / n as f32 * 2.0 - 1.0, (j as f32 + 0.5) / n as f32 * 2.0 - 1.0) as f64;
                }
            }
            (sum / (n * n) as f64) as f32
        }
    }

    #[test]
    fn a_neutral_aim_is_no_aim() {
        assert!(AimParams::default().is_neutral());
        let x = vec3f(1.0, 0.0, 0.0);
        let (right, up, half) = (vec3f(0.0, 0.0, 1.0), vec3f(0.0, 1.0, 0.0), vec2f(0.3, 0.2));
        assert!(Aim::new(&AimParams::default(), x, right, up, half, 0.2).is_none());
        // NaN reads as 0, so a broken aim is still no aim.
        let broken = AimParams { yaw_deg: f32::NAN, ..AimParams::default() };
        assert!(Aim::new(&broken, x, right, up, half, 0.2).is_none());
    }

    #[test]
    fn a_bare_face_keeps_its_brightness_until_it_is_edge_on() {
        // A diffuse face is as bright from the side as from the front.
        let turned = Face::new(AimParams { yaw_deg: 40.0, ..AimParams::default() }, 20.0);
        assert_eq!(turned.at(0.0, 0.0), 1.0);
        assert_eq!(turned.at(0.8, -0.8), 1.0);
        let edge_on = Face::new(AimParams { yaw_deg: 90.0, ..AimParams::default() }, 20.0);
        assert!(edge_on.at(0.0, 0.0) < 0.01);
    }

    #[test]
    fn a_closed_door_hides_its_half_of_the_face() {
        let top = Face::new(AimParams { barn_doors: [90.0, 0.0, 0.0, 0.0], ..AimParams::default() }, 40.0);
        assert!(top.at(0.0, 0.5) < 0.01, "the top half is behind the door");
        assert!(top.at(0.0, -0.5) > 0.99);
        assert!(top.at(0.5, -0.5) > 0.99);
        // The second door is the one on the viewer's right.
        let right = Face::new(AimParams { barn_doors: [0.0, 90.0, 0.0, 0.0], ..AimParams::default() }, 40.0);
        assert!(right.at(0.5, 0.0) < 0.01);
        assert!(right.at(-0.5, 0.0) > 0.99);
        let bottom_left = Face::new(AimParams { barn_doors: [0.0, 0.0, 90.0, 90.0], ..AimParams::default() }, 40.0);
        assert!(bottom_left.at(0.5, 0.5) > 0.99, "only the top right quarter shows");
        assert!(bottom_left.at(-0.5, 0.5) < 0.01 && bottom_left.at(0.5, -0.5) < 0.01);
    }

    #[test]
    fn a_door_standing_straight_out_hides_the_top_seen_from_above() {
        // Pitched 30 degrees down, the face is seen from above its axis, so a
        // top door standing straight out (45) hides the top of the face.
        let down = Face::new(AimParams { pitch_deg: -30.0, barn_doors: [45.0, 0.0, 0.0, 0.0], ..AimParams::default() }, 40.0);
        assert!(down.at(0.0, 0.9) < 0.01);
        assert!(down.at(0.0, -0.5) > 0.99);
        // Seen from below (pitched up), the same door hides nothing there.
        let up = Face::new(AimParams { pitch_deg: 30.0, barn_doors: [45.0, 0.0, 0.0, 0.0], ..AimParams::default() }, 40.0);
        assert!(up.at(0.0, 0.9) > 0.99);
    }

    #[test]
    fn a_grid_darkens_the_face_as_it_turns_away() {
        let means: Vec<f32> = [0.0f32, 8.0, 16.0]
            .iter()
            .map(|&yaw| Face::new(AimParams { yaw_deg: yaw, grid: 0.5, ..AimParams::default() }, 10.0).mean())
            .collect();
        assert!(means[0] > 0.6, "{means:?}");
        assert!(means[0] > means[1] && means[1] > means[2], "{means:?}");
        let cut = Face::new(AimParams { yaw_deg: 30.0, grid: 1.0, ..AimParams::default() }, 10.0);
        assert!(cut.mean() < 0.02, "a 15 degree grid turned 30 degrees away shows nothing");
        // Turned to the viewer's right, the right side of the face is the
        // further off its axis and goes dark first.
        let turned = Face::new(AimParams { yaw_deg: 10.0, grid: 0.6, ..AimParams::default() }, 30.0);
        let side = |x: f32| (0..31).map(|k| turned.at(x, (k as f32 - 15.0) / 20.0)).sum::<f32>() / 31.0;
        assert!(side(-0.7) > 0.4 && side(0.7) < 0.05, "left {}, right {}", side(-0.7), side(0.7));
    }

    #[test]
    fn the_honeycomb_cells_are_hexagons_one_unit_across() {
        // A cell is centred on the origin, and the next one sits one unit to the right.
        assert!(hex_cell(vec2f(0.0, 0.0)).length() < 1.0e-6);
        let near = hex_cell(vec2f(0.49, 0.0));
        assert!((near.x - 0.49).abs() < 1.0e-5 && near.y.abs() < 1.0e-5);
        let next = hex_cell(vec2f(0.51, 0.0));
        assert!((next.x + 0.49).abs() < 1.0e-5, "{next:?}");
        // The row above is shifted by half a cell.
        assert!(hex_cell(vec2f(0.5, 0.866_025_4)).length() < 1.0e-5);
        // Every point is within half a unit (the cell's inner radius) of its cell's edge distance.
        for k in 0..400 {
            let p = vec2f(k as f32 * 0.137 - 20.0, k as f32 * 0.291 - 37.0);
            let d = hex_dist(hex_cell(p));
            assert!(d >= 0.0 && d <= 0.5 + 1.0e-5, "{p:?}: {d}");
        }
        assert!((hex_dist(vec2f(0.5, 0.0)) - 0.5).abs() < 1.0e-6, "the flat sides face left and right");
    }
}
