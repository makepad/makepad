//! Deterministic, host-side synthetic scans with analytic ground truth in metres.
//! World y points down; cameras use OpenCV axes and row-major camera-to-world poses.
//! Geometry expects finite inputs, positive shape dimensions and nonzero focal lengths.

/// An analytic object resting on the ground at y = 0.
#[derive(Clone, Debug)]
pub enum Shape {
    /// Box centred on x = z = 0, spanning y in [-2hy, 0].
    Box {
        /// Half sizes (hx, hy, hz), in metres.
        half: [f64; 3],
    },
    /// Two boxes centred on x = z = 0, with the top resting on the base.
    Stepped {
        /// Base half sizes (hx, hy, hz), in metres.
        base: [f64; 3],
        /// Top block half sizes (hx, hy, hz), in metres.
        top: [f64; 3],
    },
    /// Sphere centred at (0, -radius, 0).
    Sphere {
        /// Radius in metres.
        radius: f64,
    },
    /// Capped vertical cylinder along y, centred on x = z = 0.
    Cylinder {
        /// Radius in metres.
        radius: f64,
        /// Height above the ground, in metres.
        height: f64,
    },
}

/// An object and an optional infinite ground plane.
pub struct Scene {
    /// Analytic object to render.
    pub shape: Shape,
    /// Whether to include the plane y = 0.
    pub ground: bool,
}

/// OpenCV pinhole camera with radial-tangential distortion.
#[derive(Clone, Debug)]
pub struct Camera {
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
    /// Horizontal focal length in pixels.
    pub fx: f64,
    /// Vertical focal length in pixels.
    pub fy: f64,
    /// Horizontal principal point in pixels.
    pub cx: f64,
    /// Vertical principal point in pixels.
    pub cy: f64,
    /// Skew coefficient in pixels.
    pub skew: f64,
    /// OpenCV distortion coefficients: k1, k2, p1, p2, k3.
    pub distortion: [f64; 5],
    /// Row-major rigid camera-to-world transform.
    pub cam_to_world: [[f64; 4]; 4],
}

/// Row-major rendered images, indexed by y * width + x.
pub struct Render {
    /// Interleaved RGB8 texture and Lambert shading.
    pub rgb: Vec<u8>,
    /// Camera-frame +z depth in metres; zero on a miss.
    pub depth_z: Vec<f32>,
    /// Interleaved unit camera-frame rays through undistorted pixel centres.
    pub rays: Vec<f32>,
    /// Object = 1, ground = 2, nothing = 0.
    pub hit: Vec<u8>,
}

/// Unsigned surface error in millimetres; all fields are zero for no valid points.
#[derive(Debug)]
pub struct ErrorStats {
    /// Number of valid points scored.
    pub count: usize,
    /// Arithmetic mean error in millimetres.
    pub mean_mm: f64,
    /// Nearest-rank 95th percentile error in millimetres.
    pub p95_mm: f64,
    /// Maximum error in millimetres.
    pub max_mm: f64,
}

impl Camera {
    /// Maps already-distorted normalised coordinates to pixels, including skew.
    pub fn project_normalized(&self, x: f64, y: f64) -> [f64; 2] {
        [self.fx * x + self.skew * y + self.cx, self.fy * y + self.cy]
    }

    /// Inverts pinhole and skew, then fixed-point undistorts to coordinates with z = 1.
    /// Performs at least 20 steps and stops at a step below 1e-12.
    /// Like OpenCV's fixed-point inversion, this expects an invertible lens model
    /// in its convergent field of view.
    pub fn unproject(&self, u: f64, v: f64) -> [f64; 2] {
        let yd = (v - self.cy) / self.fy;
        let xd = (u - self.cx - self.skew * yd) / self.fx;
        let [k1, k2, p1, p2, k3] = self.distortion;
        let [mut x, mut y] = [xd, yd];
        let mut minimum_steps = 20_usize;
        let mut step = f64::INFINITY;
        while minimum_steps > 0 || step >= 1e-12 {
            let r2 = x * x + y * y;
            let radial = 1.0 + r2 * (k1 + r2 * (k2 + r2 * k3));
            let next_x = (xd - 2.0 * p1 * x * y - p2 * (r2 + 2.0 * x * x)) / radial;
            let next_y = (yd - p1 * (r2 + 2.0 * y * y) - 2.0 * p2 * x * y) / radial;
            step = (next_x - x).hypot(next_y - y);
            [x, y] = [next_x, next_y];
            minimum_steps = minimum_steps.saturating_sub(1);
        }
        [x, y]
    }
}

/// Applies OpenCV radial-tangential distortion to undistorted normalised coordinates.
pub fn distort(d: [f64; 5], x: f64, y: f64) -> [f64; 2] {
    let [k1, k2, p1, p2, k3] = d;
    let r2 = x * x + y * y;
    let radial = 1.0 + r2 * (k1 + r2 * (k2 + r2 * k3));
    [
        x * radial + 2.0 * p1 * x * y + p2 * (r2 + 2.0 * x * x),
        y * radial + p1 * (r2 + 2.0 * y * y) + 2.0 * p2 * x * y,
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    v.map(|x| x / length)
}

fn rotate(pose: &[[f64; 4]; 4], v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| pose[i][0] * v[0] + pose[i][1] * v[1] + pose[i][2] * v[2])
}

fn origin(pose: &[[f64; 4]; 4]) -> [f64; 3] {
    std::array::from_fn(|i| pose[i][3])
}

fn along(o: [f64; 3], d: [f64; 3], t: f64) -> [f64; 3] {
    std::array::from_fn(|i| o[i] + t * d[i])
}

/// Camera-to-world pose with z toward target, x = normalise(z cross up), y = z cross x,
/// so the image's down (camera +y) leans toward the world's down (+y) and the camera is
/// upright. Up is (0, -1, 0); eye and target must differ and not be vertically aligned.
pub fn look_at(eye: [f64; 3], target: [f64; 3]) -> [[f64; 4]; 4] {
    let z = unit(sub(target, eye));
    let x = unit(cross(z, [0.0, -1.0, 0.0]));
    let y = cross(z, x);
    [
        [x[0], y[0], z[0], eye[0]],
        [x[1], y[1], z[1], eye[1]],
        [x[2], y[2], z[2], eye[2]],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / ((1_u64 << 53) as f64)
    }

    fn signed(&mut self) -> f64 {
        2.0 * self.next() - 1.0
    }
}

/// Copies camera intrinsics to evenly spaced poses on a horizontal circle.
/// Angles are start + arc * i / count (no duplicated endpoint for a full circle).
/// Eyes are (radius*cos(angle), -elevation, radius*sin(angle)); all aim at
/// (0, -target_height, 0) before seeded LCG noise. Rotation noise is applied on
/// the right in camera coordinates; translation noise is per world axis.
pub fn orbit(
    camera: &Camera,
    count: usize,
    radius: f64,
    elevation: f64,
    target_height: f64,
    start: f64,
    arc: f64,
    rot_noise: f64,
    trans_noise: f64,
    seed: u64,
) -> Vec<Camera> {
    let mut random = Lcg(seed);
    (0..count)
        .map(|i| {
            let angle = start + arc * i as f64 / count as f64;
            let mut pose = look_at(
                [radius * angle.cos(), -elevation, radius * angle.sin()],
                [0.0, -target_height, 0.0],
            );
            // Uniform sphere axis, followed by Rodrigues' axis-angle rotation.
            let azimuth = std::f64::consts::TAU * random.next();
            let z = random.signed();
            let radial = (1.0 - z * z).sqrt();
            let axis = [radial * azimuth.cos(), radial * azimuth.sin(), z];
            let theta = rot_noise * random.signed();
            let (s, c) = theta.sin_cos();
            let old = pose;
            for j in 0..3 {
                let basis = std::array::from_fn(|k| if k == j { 1.0 } else { 0.0 });
                let skew = cross(axis, basis);
                let column = std::array::from_fn(|k| {
                    c * basis[k] + (1.0 - c) * axis[k] * axis[j] + s * skew[k]
                });
                let column = rotate(&old, column);
                for k in 0..3 {
                    pose[k][j] = column[k];
                }
            }
            for row in &mut pose[..3] {
                row[3] += trans_noise * random.signed();
            }
            Camera {
                cam_to_world: pose,
                ..camera.clone()
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
struct Bounds {
    lo: [f64; 3],
    hi: [f64; 3],
}

fn box_bounds(half: [f64; 3], bottom: f64) -> Bounds {
    Bounds {
        lo: [-half[0], bottom - 2.0 * half[1], -half[2]],
        hi: [half[0], bottom, half[2]],
    }
}

#[derive(Clone, Copy)]
struct Face {
    bounds: Bounds,
    axis: usize,
    sign: f64,
}

impl Face {
    fn distance(&self, p: [f64; 3]) -> f64 {
        let q = std::array::from_fn(|i| p[i].clamp(self.bounds.lo[i], self.bounds.hi[i]));
        let delta = sub(p, q);
        dot(delta, delta).sqrt()
    }

    fn intersect(&self, o: [f64; 3], d: [f64; 3], hit: &mut Hit) {
        if d[self.axis] == 0.0 {
            return;
        }
        let t = (self.bounds.lo[self.axis] - o[self.axis]) / d[self.axis];
        let p = along(o, d, t);
        if (0..3)
            .filter(|&i| i != self.axis)
            .all(|i| p[i] >= self.bounds.lo[i] && p[i] <= self.bounds.hi[i])
        {
            let mut normal = [0.0; 3];
            normal[self.axis] = self.sign;
            hit.consider(t, normal, 1);
        }
    }
}

fn box_faces(bounds: Bounds, other: Option<Bounds>, emit: &mut impl FnMut(Face)) {
    for axis in 0..3 {
        for sign in [-1.0, 1.0] {
            let coordinate = if sign < 0.0 {
                bounds.lo[axis]
            } else {
                bounds.hi[axis]
            };
            let mut face = Face { bounds, axis, sign };
            face.bounds.lo[axis] = coordinate;
            face.bounds.hi[axis] = coordinate;
            let a = (axis + 1) % 3;
            let b = (axis + 2) % 3;
            if let Some(other) = other {
                // Include contact planes: a coincident face with solid on its
                // outward side is internal to the union, even though neither
                // box's strict interior contains a point on the shared plane.
                let covers = if sign < 0.0 {
                    other.lo[axis] < coordinate && other.hi[axis] >= coordinate
                } else {
                    other.lo[axis] <= coordinate && other.hi[axis] > coordinate
                };
                let low_a = bounds.lo[a].max(other.lo[a]);
                let high_a = bounds.hi[a].min(other.hi[a]);
                let low_b = bounds.lo[b].max(other.lo[b]);
                let high_b = bounds.hi[b].min(other.hi[b]);
                if covers && low_a < high_a && low_b < high_b {
                    // Rectangle minus covered rectangle, as four disjoint
                    // strips. Project onto each strip, so covered projections
                    // still find the nearest exposed edge without sampling.
                    for (al, ah, bl, bh) in [
                        (bounds.lo[a], low_a, bounds.lo[b], bounds.hi[b]),
                        (high_a, bounds.hi[a], bounds.lo[b], bounds.hi[b]),
                        (low_a, high_a, bounds.lo[b], low_b),
                        (low_a, high_a, high_b, bounds.hi[b]),
                    ] {
                        if al < ah && bl < bh {
                            let mut strip = face;
                            strip.bounds.lo[a] = al;
                            strip.bounds.hi[a] = ah;
                            strip.bounds.lo[b] = bl;
                            strip.bounds.hi[b] = bh;
                            emit(strip);
                        }
                    }
                    continue;
                }
            }
            emit(face);
        }
    }
}

fn shape_faces(shape: &Shape, mut emit: impl FnMut(Face)) {
    match *shape {
        Shape::Box { half } => box_faces(box_bounds(half, 0.0), None, &mut emit),
        Shape::Stepped { base, top } => {
            let a = box_bounds(base, 0.0);
            let b = box_bounds(top, a.lo[1]);
            box_faces(a, Some(b), &mut emit);
            box_faces(b, Some(a), &mut emit);
        }
        _ => {}
    }
}

struct Hit {
    t: f64,
    normal: [f64; 3],
    kind: u8,
}

impl Hit {
    fn consider(&mut self, t: f64, normal: [f64; 3], kind: u8) {
        if t > 1e-9 && t < self.t {
            *self = Self { t, normal, kind };
        }
    }
}

// Roots of a*t^2 + 2*b*t + c, ordered and avoiding cancellation.
fn roots(a: f64, b: f64, c: f64) -> Option<[f64; 2]> {
    let discriminant = b * b - a * c;
    if a == 0.0 || discriminant < 0.0 {
        return None;
    }
    let q = -b - discriminant.sqrt().copysign(b);
    if q == 0.0 {
        return Some([-b / a; 2]);
    }
    let x = q / a;
    let y = c / q;
    Some([x.min(y), x.max(y)])
}

/// Shaded colour per channel in [0, 255], before rounding.
fn texture(p: [f64; 3], n: [f64; 3], light: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = p;
    let hash = |x: f64, y: f64, z: f64| {
        let v = (x * 127.1 + y * 311.7 + z * 74.7).sin() * 43758.5453;
        v - v.floor()
    };
    let fine = hash(
        (400.0 * x).floor(),
        (400.0 * y).floor(),
        (400.0 * z).floor(),
    );
    let mid = 0.5 + 0.5 * (90.0 * x).sin() * (70.0 * y + 1.0).sin() * (80.0 * z + 2.0).sin();
    let r = 0.35 + 0.45 * fine * mid + 0.2 * (0.5 + 0.5 * (23.0 * x).sin());
    let g = 0.30
        + 0.50
            * hash(
                (150.0 * x).floor(),
                (150.0 * y).floor() + 7.0,
                (150.0 * z).floor(),
            );
    let b = 0.25 + 0.45 * mid;
    let lambert = 0.35 + 0.65 * dot(n, light).max(0.0);
    [r, g, b].map(|v| (v * lambert).clamp(0.0, 1.0) * 255.0)
}

/// Colour of a ray that hits nothing.
const BACKGROUND: f64 = 26.0;

/// How the object's surface reflects light. The ground always keeps the world texture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// The deterministic world texture with Lambert shading.
    Textured,
    /// One uniform albedo with Lambert shading only, like smooth plastic or paint.
    Uniform,
    /// `Uniform` plus a mild Blinn-Phong highlight that moves with the viewpoint.
    Glossy,
}

/// Render settings beyond the scene and the camera.
pub struct RenderOptions {
    /// Rays per pixel along each axis; a pixel's colour is their mean (1: the pixel centre only).
    pub supersample: usize,
    /// The object's surface.
    pub surface: Surface,
}

/// Albedo of `Surface::Uniform` and `Surface::Glossy`.
const UNIFORM_ALBEDO: [f64; 3] = [0.62, 0.58, 0.52];
/// Highlight strength and exponent of `Surface::Glossy`.
const GLOSS: f64 = 0.25;
const SHININESS: f64 = 16.0;

/// Colour per channel in [0, 255] of a hit seen along the unit world direction `d`.
fn shade(surface: Surface, hit: &Hit, p: [f64; 3], d: [f64; 3], light: [f64; 3]) -> [f64; 3] {
    if hit.kind != 1 || surface == Surface::Textured {
        return texture(p, hit.normal, light);
    }
    let lambert = 0.35 + 0.65 * dot(hit.normal, light).max(0.0);
    let highlight = if surface == Surface::Glossy {
        let half = unit([light[0] - d[0], light[1] - d[1], light[2] - d[2]]);
        GLOSS * dot(hit.normal, half).max(0.0).powf(SHININESS)
    } else {
        0.0
    };
    UNIFORM_ALBEDO.map(|a| (a * lambert + highlight).clamp(0.0, 1.0) * 255.0)
}

/// The first surface along the world ray from `o` in direction `d`.
fn trace(scene: &Scene, faces: &[Face], o: [f64; 3], d: [f64; 3]) -> Hit {
    let mut hit = Hit {
        t: f64::INFINITY,
        normal: [0.0; 3],
        kind: 0,
    };
    for face in faces {
        face.intersect(o, d, &mut hit);
    }
    match scene.shape {
        Shape::Sphere { radius } => {
            let offset = sub(o, [0.0, -radius, 0.0]);
            if let Some(ts) = roots(
                dot(d, d),
                dot(offset, d),
                dot(offset, offset) - radius * radius,
            ) {
                for t in ts {
                    hit.consider(t, unit(along(offset, d, t)), 1);
                }
            }
        }
        Shape::Cylinder { radius, height } => {
            if let Some(ts) = roots(
                d[0] * d[0] + d[2] * d[2],
                o[0] * d[0] + o[2] * d[2],
                o[0] * o[0] + o[2] * o[2] - radius * radius,
            ) {
                for t in ts {
                    let p = along(o, d, t);
                    if p[1] >= -height && p[1] <= 0.0 {
                        hit.consider(t, unit([p[0], 0.0, p[2]]), 1);
                    }
                }
            }
            if d[1] != 0.0 {
                for (level, sign) in [(-height, -1.0), (0.0, 1.0)] {
                    let t = (level - o[1]) / d[1];
                    let p = along(o, d, t);
                    if p[0] * p[0] + p[2] * p[2] <= radius * radius {
                        hit.consider(t, [0.0, sign, 0.0], 1);
                    }
                }
            }
        }
        _ => {}
    }
    if scene.ground && d[1] != 0.0 {
        hit.consider(-o[1] / d[1], [0.0, -1.0, 0.0], 2);
    }
    hit
}

/// Ray-casts the first surface at t > 1e-9 and shades it with deterministic world texture.
/// Work and output memory are linear in pixel count; there is no GPU or UI work.
pub fn render(scene: &Scene, camera: &Camera) -> Render {
    render_supersampled(scene, camera, 1)
}

/// [`render`] with each pixel's colour the mean of `factor` x `factor` shaded rays through the pixel at offsets
/// ((i + 0.5) / factor, (j + 0.5) / factor), rounded once, as a camera integrates light over its pixel area;
/// rays that miss count as the background colour. Depth, hit and ray stay those of the pixel-centre ray, so the
/// truth is unchanged. `factor` 1 gives exactly [`render`]. Panics if `factor` is 0.
pub fn render_supersampled(scene: &Scene, camera: &Camera, factor: usize) -> Render {
    render_with(scene, camera, &RenderOptions { supersample: factor, surface: Surface::Textured })
}

/// [`render_supersampled`] with `options.supersample` rays per axis and the object's surface from
/// `options.surface`; depth, hit and rays do not depend on either. Panics if `options.supersample` is 0.
pub fn render_with(scene: &Scene, camera: &Camera, options: &RenderOptions) -> Render {
    let factor = options.supersample;
    assert!(factor > 0, "render_with needs at least one ray per pixel");
    let pixels = camera.width * camera.height;
    let mut result = Render {
        rgb: vec![0; 3 * pixels],
        depth_z: vec![0.0; pixels],
        rays: vec![0.0; 3 * pixels],
        hit: vec![0; pixels],
    };
    let mut faces = Vec::new();
    shape_faces(&scene.shape, |face| faces.push(face));
    let o = origin(&camera.cam_to_world);
    let light = unit([0.3, -1.0, -0.4]);
    let samples = (factor * factor) as f64;
    for y in 0..camera.height {
        for x in 0..camera.width {
            let index = y * camera.width + x;
            let [xn, yn] = camera.unproject(x as f64 + 0.5, y as f64 + 0.5);
            let ray = unit([xn, yn, 1.0]);
            result.rays[3 * index..3 * index + 3].copy_from_slice(&ray.map(|v| v as f32));
            let hit = trace(scene, &faces, o, rotate(&camera.cam_to_world, ray));
            if hit.kind != 0 {
                result.depth_z[index] = (hit.t * ray[2]) as f32;
                result.hit[index] = hit.kind;
            }
            let mut colour = [0.0; 3];
            for j in 0..factor {
                for i in 0..factor {
                    let [sx, sy] = if factor == 1 {
                        [xn, yn]
                    } else {
                        camera.unproject(x as f64 + (i as f64 + 0.5) / factor as f64, y as f64 + (j as f64 + 0.5) / factor as f64)
                    };
                    let d = rotate(&camera.cam_to_world, unit([sx, sy, 1.0]));
                    let sample = trace(scene, &faces, o, d);
                    let shaded = if sample.kind != 0 {
                        shade(options.surface, &sample, along(o, d, sample.t), d, light)
                    } else {
                        [BACKGROUND; 3]
                    };
                    for (c, v) in colour.iter_mut().zip(shaded) {
                        *c += v;
                    }
                }
            }
            for (out, c) in result.rgb[3 * index..3 * index + 3].iter_mut().zip(colour) {
                *out = (c / samples).round() as u8;
            }
        }
    }
    result
}

/// Exact unsigned distance to the object's boundary, excluding the ground.
/// Stepped boxes project onto exposed face rectangles, including their edges,
/// with the internal contact region removed analytically (no surface samples).
pub fn distance_to_shape(shape: &Shape, p: [f64; 3]) -> f64 {
    match *shape {
        Shape::Box { half } => {
            let q = [
                p[0].abs() - half[0],
                (p[1] + half[1]).abs() - half[1],
                p[2].abs() - half[2],
            ];
            let outside = q.map(|v| v.max(0.0));
            (dot(outside, outside).sqrt() + q[0].max(q[1]).max(q[2]).min(0.0)).abs()
        }
        Shape::Stepped { .. } => {
            let mut distance = f64::INFINITY;
            shape_faces(shape, |face| distance = distance.min(face.distance(p)));
            distance
        }
        Shape::Sphere { radius } => {
            let offset = sub(p, [0.0, -radius, 0.0]);
            (dot(offset, offset).sqrt() - radius).abs()
        }
        Shape::Cylinder { radius, height } => {
            let radial = p[0].hypot(p[2]) - radius;
            let vertical = (p[1] + height * 0.5).abs() - height * 0.5;
            (radial.max(0.0).hypot(vertical.max(0.0)) + radial.max(vertical).min(0.0)).abs()
        }
    }
}

/// Computes millimetre errors for flagged points, with nearest-rank p95.
/// Missing validity flags count as false; surplus flags are unused.
pub fn error_stats(shape: &Shape, points: &[[f64; 3]], valid: &[bool]) -> ErrorStats {
    let mut errors: Vec<f64> = points
        .iter()
        .zip(valid)
        .filter(|(_, flag)| **flag)
        .map(|(&p, _)| 1000.0 * distance_to_shape(shape, p))
        .collect();
    let count = errors.len();
    if count == 0 {
        return ErrorStats {
            count: 0,
            mean_mm: 0.0,
            p95_mm: 0.0,
            max_mm: 0.0,
        };
    }
    let mean_mm = errors.iter().sum::<f64>() / count as f64;
    errors.sort_by(f64::total_cmp);
    // ceil(0.95*n) - 1 = n - floor(n/20) - 1, without multiplication overflow.
    ErrorStats {
        count,
        mean_mm,
        p95_mm: errors[count - count / 20 - 1],
        max_mm: errors[count - 1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera {
        Camera {
            width: 518,
            height: 518,
            fx: 400.0,
            fy: 400.0,
            cx: 259.0,
            cy: 259.0,
            skew: 0.0,
            distortion: [0.0; 5],
            cam_to_world: look_at([0.0, -0.04, -0.4], [0.0, -0.04, 0.0]),
        }
    }

    fn shapes() -> [Shape; 4] {
        [
            Shape::Box {
                half: [0.05, 0.04, 0.03],
            },
            Shape::Stepped {
                base: [0.06, 0.02, 0.05],
                top: [0.03, 0.03, 0.025],
            },
            Shape::Sphere { radius: 0.05 },
            Shape::Cylinder {
                radius: 0.05,
                height: 0.10,
            },
        ]
    }

    fn close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() < tolerance,
            "actual {actual}, expected {expected}, tolerance {tolerance}"
        );
    }

    fn frame(pose: &[[f64; 4]; 4]) {
        let columns: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| pose[j][i]));
        for i in 0..3 {
            for j in 0..3 {
                close(
                    dot(columns[i], columns[j]),
                    if i == j { 1.0 } else { 0.0 },
                    1e-12,
                );
            }
        }
        close(dot(cross(columns[0], columns[1]), columns[2]), 1.0, 1e-12);
        assert_eq!(pose[3], [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn distortion_and_skew_round_trip_over_image() {
        let c = Camera {
            distortion: [-0.28, 0.08, 1e-4, -2e-4, -0.01],
            skew: 0.5,
            ..camera()
        };
        for y in 0..10 {
            for x in 0..20 {
                let xn = ((x as f64 + 0.5) * c.width as f64 / 20.0 - c.cx) / c.fx;
                let yn = ((y as f64 + 0.5) * c.height as f64 / 10.0 - c.cy) / c.fy;
                let [xd, yd] = distort(c.distortion, xn, yn);
                let [u, v] = c.project_normalized(xd, yd);
                let result = c.unproject(u, v);
                close(result[0], xn, 1e-9);
                close(result[1], yn, 1e-9);
            }
        }
    }

    #[test]
    fn look_at_aims_at_target_with_right_handed_orthonormal_frame() {
        let eye = [0.3, -0.2, -0.4];
        let target = [0.0, -0.05, 0.0];
        let pose = look_at(eye, target);
        frame(&pose);
        let expected = unit(sub(target, eye));
        for i in 0..3 {
            close(pose[i][2], expected[i], 1e-12);
            close(pose[i][3], eye[i], 1e-12);
        }
        let expected_x = unit(cross(expected, [0.0, -1.0, 0.0]));
        for i in 0..3 {
            close(pose[i][0], expected_x[i], 1e-12);
        }
    }

    #[test]
    fn look_at_cameras_are_upright_so_the_near_ground_is_at_the_image_bottom() {
        // Looking straight along +z from above the ground: image x is world +x, image down is world down.
        let level = look_at([0.0, -0.2, -0.4], [0.0, -0.2, 0.0]);
        for (column, expected) in [(0, [1.0, 0.0, 0.0]), (1, [0.0, 1.0, 0.0])] {
            for row in 0..3 {
                close(level[row][column], expected[row], 1e-12);
            }
        }
        // An orbit camera looking down at the target: its down axis leans to world down, and
        // the ground nearest the camera fills the bottom rows of the image.
        let c = camera();
        let cameras = orbit(&c, 3, 0.4, 0.25, 0.04, 0.0, std::f64::consts::TAU, 0.0, 0.0, 1);
        let scene = Scene {
            shape: Shape::Sphere { radius: 0.01 },
            ground: true,
        };
        for camera in &cameras {
            assert!(camera.cam_to_world[1][1] > 0.0, "image down is world down");
            let output = render(&scene, camera);
            let x = c.width / 2;
            let bottom = (c.height - 1) * c.width + x;
            let top = x;
            assert_eq!(output.hit[bottom], 2, "the bottom row sees the ground");
            assert!(
                output.hit[top] == 0 || output.depth_z[top] > output.depth_z[bottom],
                "the top row sees farther than the bottom row"
            );
        }
    }

    #[test]
    fn front_box_centre_depth_is_camera_z_not_ray_length() {
        let c = camera();
        let scene = Scene {
            shape: Shape::Box {
                half: [0.05, 0.04, 0.03],
            },
            ground: false,
        };
        let output = render(&scene, &c);
        let centre = 259 * c.width + 259;
        assert_eq!(output.hit[centre], 1);
        close(f64::from(output.depth_z[centre]), 0.37, 1e-6);
        assert_eq!(output.hit[0], 0);
        assert_eq!(output.depth_z[0], 0.0);
        assert_eq!(&output.rgb[..3], &[26, 26, 26]);
        // Also check an oblique hit, where z depth differs visibly from t.
        let edge = 259 * c.width + 300;
        assert_eq!(output.hit[edge], 1);
        close(f64::from(output.depth_z[edge]), 0.37, 1e-6);
    }

    #[test]
    fn rendered_hits_of_all_shapes_reconstruct_on_exact_surfaces() {
        let c = Camera {
            width: 48,
            height: 48,
            fx: 60.0,
            fy: 60.0,
            cx: 24.0,
            cy: 24.0,
            skew: 0.5,
            distortion: [-0.28, 0.08, 1e-4, -2e-4, -0.01],
            ..camera()
        };
        let cameras = orbit(
            &c,
            4,
            0.3,
            0.19,
            0.05,
            0.2,
            std::f64::consts::TAU,
            0.0,
            0.0,
            1,
        );
        for shape in shapes() {
            let scene = Scene {
                shape,
                ground: true,
            };
            for c in &cameras {
                let output = render(&scene, c);
                let mut hits = 0;
                let mut ground = 0;
                for (i, &kind) in output.hit.iter().enumerate() {
                    let ray = [
                        output.rays[3 * i],
                        output.rays[3 * i + 1],
                        output.rays[3 * i + 2],
                    ]
                    .map(f64::from);
                    close(dot(ray, ray), 1.0, 1e-6);
                    if kind == 0 {
                        continue;
                    }
                    let p = along(
                        origin(&c.cam_to_world),
                        rotate(&c.cam_to_world, ray),
                        f64::from(output.depth_z[i]) / ray[2],
                    );
                    if kind == 1 {
                        hits += 1;
                        let error = distance_to_shape(&scene.shape, p);
                        assert!(
                            error < 1e-6,
                            "shape {:?}, point {p:?}, error {error}",
                            scene.shape
                        );
                    } else {
                        ground += 1;
                        close(p[1], 0.0, 1e-6);
                    }
                }
                assert!(hits > 0, "object must be visible: {:?}", scene.shape);
                assert!(ground > 0, "ground must be visible");
            }
        }
    }

    #[test]
    fn surface_distance_is_exact_and_excludes_internal_step_contact() {
        for (shape, top) in shapes().iter().zip([0.08, 0.10, 0.10, 0.10]) {
            close(
                distance_to_shape(shape, [0.0, -top - 0.01, 0.0]),
                0.01,
                1e-12,
            );
        }
        let stepped = &shapes()[1];
        let inside = distance_to_shape(stepped, [0.0, -0.04, 0.0]);
        assert!(inside > 0.0, "contact plane is not the union boundary");
        close(inside, 0.025, 1e-12);
        close(distance_to_shape(stepped, [0.04, -0.04, 0.0]), 0.0, 1e-12);
        close(
            distance_to_shape(stepped, [0.0, -0.03, 0.0]),
            0.025_f64.hypot(0.01),
            1e-12,
        );
        // Equal footprints must erase the whole contact face; overhanging tops
        // expose their undersides outside the base footprint.
        let equal = Shape::Stepped {
            base: [0.05, 0.02, 0.05],
            top: [0.05, 0.03, 0.05],
        };
        close(distance_to_shape(&equal, [0.0, -0.04, 0.0]), 0.04, 1e-12);
        let overhang = Shape::Stepped {
            base: [0.02, 0.02, 0.02],
            top: [0.05, 0.03, 0.05],
        };
        close(distance_to_shape(&overhang, [0.04, -0.04, 0.0]), 0.0, 1e-12);
        close(distance_to_shape(&overhang, [0.0, -0.04, 0.0]), 0.02, 1e-12);
    }

    #[test]
    fn error_statistics_measure_millimetres_and_nearest_rank_on_valid_points() {
        for (shape, top) in shapes().iter().zip([0.08, 0.10, 0.10, 0.10]) {
            let points = [[0.0, -top - 0.001, 0.0], [0.0, 0.001, 0.0], [10.0; 3]];
            let stats = error_stats(shape, &points, &[true, true, false]);
            assert_eq!(stats.count, 2);
            close(stats.mean_mm, 1.0, 1e-6);
            close(stats.p95_mm, 1.0, 1e-6);
            close(stats.max_mm, 1.0, 1e-6);
        }
        let shape = &shapes()[0];
        let points: Vec<_> = (1..=20)
            .rev()
            .map(|i| [0.0, -0.08 - i as f64 * 0.001, 0.0])
            .collect();
        let stats = error_stats(shape, &points, &[true; 20]);
        assert_eq!(stats.count, 20);
        close(stats.mean_mm, 10.5, 1e-9);
        close(stats.p95_mm, 19.0, 1e-9);
        close(stats.max_mm, 20.0, 1e-9);
        let empty = error_stats(shape, &points, &[false; 20]);
        assert_eq!(empty.count, 0);
        assert_eq!([empty.mean_mm, empty.p95_mm, empty.max_mm], [0.0; 3]);
    }

    #[test]
    fn orbit_preserves_geometry_and_seeded_noise_preserves_rotations() {
        let c = camera();
        let make = |r, t, seed| {
            orbit(
                &c,
                9,
                0.4,
                0.2,
                0.05,
                0.3,
                std::f64::consts::TAU,
                r,
                t,
                seed,
            )
        };
        let clean = make(0.0, 0.0, 42);
        for (i, camera) in clean.iter().enumerate() {
            let pose = &camera.cam_to_world;
            frame(pose);
            let eye = origin(pose);
            close(eye[0].hypot(eye[2]), 0.4, 1e-12);
            close(eye[1], -0.2, 1e-12);
            let angle = 0.3 + std::f64::consts::TAU * i as f64 / 9.0;
            close(eye[0], 0.4 * angle.cos(), 1e-12);
            close(eye[2], 0.4 * angle.sin(), 1e-12);
            let aim = unit(sub([0.0, -0.05, 0.0], eye));
            for k in 0..3 {
                close(pose[k][2], aim[k], 1e-12);
            }
            assert_eq!(camera.distortion, c.distortion);
            assert_eq!((camera.width, camera.height), (c.width, c.height));
            assert_eq!(
                [camera.fx, camera.fy, camera.cx, camera.cy, camera.skew],
                [c.fx, c.fy, c.cx, c.cy, c.skew]
            );
        }
        let a = make(0.02, 0.003, 42);
        let b = make(0.02, 0.003, 42);
        let other = make(0.02, 0.003, 43);
        for i in 0..a.len() {
            assert_eq!(a[i].cam_to_world, b[i].cam_to_world);
            assert_ne!(a[i].cam_to_world, other[i].cam_to_world);
            frame(&a[i].cam_to_world);
            for k in 0..3 {
                assert!((a[i].cam_to_world[k][3] - clean[i].cam_to_world[k][3]).abs() <= 0.003);
            }
            let trace: f64 = (0..3)
                .map(|j| {
                    (0..3)
                        .map(|k| clean[i].cam_to_world[k][j] * a[i].cam_to_world[k][j])
                        .sum::<f64>()
                })
                .sum();
            assert!(((trace - 1.0) * 0.5).clamp(-1.0, 1.0).acos() <= 0.02 + 1e-12);
        }
        assert!(orbit(&c, 0, 0.4, 0.2, 0.05, 0.0, 1.0, 0.0, 0.0, 0).is_empty());
    }

    #[test]
    fn supersampled_render_keeps_the_centre_truth_and_averages_colour_over_each_pixel() {
        let scene = Scene { shape: shapes()[0].clone(), ground: true };
        let single = render(&scene, &camera());
        let fine = render_supersampled(&scene, &camera(), 4);
        assert_eq!(single.depth_z, fine.depth_z);
        assert_eq!(single.hit, fine.hit);
        assert_eq!(single.rays, fine.rays);
        // The 2.5 mm texture cells at about 1 mm pixels alias under point sampling; the area mean differs often
        // but agrees on average.
        let changed = single.rgb.iter().zip(&fine.rgb).filter(|(a, b)| a != b).count();
        assert!(changed > single.rgb.len() / 10, "only {changed} channels changed");
        let mean = |rgb: &[u8]| rgb.iter().map(|&v| f64::from(v)).sum::<f64>() / rgb.len() as f64;
        close(mean(&single.rgb), mean(&fine.rgb), 1.0);
    }

    #[test]
    fn uniform_and_glossy_surfaces_shade_only_the_object_and_keep_the_truth() {
        let scene = Scene { shape: shapes()[0].clone(), ground: true };
        let options = |surface| RenderOptions { supersample: 1, surface };
        let textured = render_with(&scene, &camera(), &options(Surface::Textured));
        let uniform = render_with(&scene, &camera(), &options(Surface::Uniform));
        let glossy = render_with(&scene, &camera(), &options(Surface::Glossy));
        assert_eq!(textured.rgb, render(&scene, &camera()).rgb);
        let mut uniform_colours = std::collections::BTreeSet::new();
        for (i, &hit) in textured.hit.iter().enumerate() {
            let pixel = |r: &Render| [r.rgb[3 * i], r.rgb[3 * i + 1], r.rgb[3 * i + 2]];
            if hit == 1 {
                uniform_colours.insert(pixel(&uniform));
                assert!(pixel(&glossy).iter().zip(pixel(&uniform)).all(|(g, u)| *g >= u), "a highlight only adds light");
            } else {
                assert_eq!(pixel(&uniform), pixel(&textured), "the ground and background keep their look");
                assert_eq!(pixel(&glossy), pixel(&textured), "the ground and background keep their look");
            }
        }
        // A box under one directional light shows at most three flat faces, each a single colour (the glossy
        // ones too: a flat face's highlight is constant across it).
        assert!(uniform_colours.len() <= 3, "{} uniform object colours", uniform_colours.len());
        for other in [&uniform, &glossy] {
            assert_eq!(other.depth_z, textured.depth_z);
            assert_eq!(other.hit, textured.hit);
        }
        let sphere = Scene { shape: shapes()[2].clone(), ground: true };
        let matte = render_with(&sphere, &camera(), &options(Surface::Uniform));
        let shiny = render_with(&sphere, &camera(), &options(Surface::Glossy));
        let brightest = matte.rgb.iter().zip(&shiny.rgb).map(|(m, s)| s - m).max().unwrap_or(0);
        assert!(brightest > 30, "the glossy sphere shows a highlight, brightest gain {brightest}");
    }
}
