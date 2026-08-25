//! The renderer's input: one flat snapshot of a scene.
//!
//! App-agnostic on purpose. A model format `Scene`, a GLB, or a hand-built Cornell
//! box all reduce to the same triangle soup + material table + camera + sun.
//! Nothing here touches the GPU; `gpu.rs` packs a `SceneInput` into data
//! textures and `bvh.rs` builds the acceleration structure over it.
//!
//! Coordinate law: whatever the caller uses. The only orientation the tracer
//! needs is `up` (for the sky dome and the sun); a model format scene passes +Z, a
//! glTF passes +Y. Lengths are in scene units; a metre is the intended one.

use makepad_draw::*;

/// One physically based material. Roughness/metal follow glTF; `transmission`
/// turns the surface into THIN glass (a window pane: Fresnel reflection off
/// the front, the rest passes straight through tinted by `albedo`) — the
/// right model for a building's glazing, which is never a closed solid.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Linear-space base colour (the tint for glass).
    pub albedo: [f32; 3],
    /// Perceptual roughness 0..1 (GGX alpha = r*r).
    pub roughness: f32,
    /// 0 dielectric, 1 metal.
    pub metal: f32,
    /// Emitted radiance (linear RGB). Non-zero makes every triangle of the
    /// material a light the integrator samples explicitly.
    pub emission: [f32; 3],
    /// Index of refraction for the Fresnel term (1.5 = glass).
    pub ior: f32,
    /// 0 opaque .. 1 thin glass.
    pub transmission: f32,
    /// Base-colour image (index into `SceneInput::images`), multiplied with `albedo`.
    pub texture: Option<usize>,
    /// Explicit sidedness. Opaque architectural faces are front-facing;
    /// glazing and deliberately thin sheets are two-sided.
    pub two_sided: bool,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            albedo: [0.8, 0.8, 0.8],
            roughness: 0.6,
            metal: 0.0,
            emission: [0.0; 3],
            ior: 1.5,
            transmission: 0.0,
            texture: None,
            two_sided: false,
        }
    }
}

impl Material {
    pub fn diffuse(rgb: [f32; 3]) -> Self {
        Self { albedo: rgb, roughness: 1.0, ..Default::default() }
    }
    pub fn emissive(rgb: [f32; 3], strength: f32) -> Self {
        Self {
            albedo: [0.0; 3],
            emission: [rgb[0] * strength, rgb[1] * strength, rgb[2] * strength],
            ..Default::default()
        }
    }
    pub fn glass(tint: [f32; 3]) -> Self {
        Self { albedo: tint, roughness: 0.0, transmission: 1.0, ior: 1.5, two_sided: true, ..Default::default() }
    }
    pub fn is_emissive(&self) -> bool {
        self.emission.iter().any(|&e| e > 0.0)
    }
}

/// An 8-bit RGBA image for the texture atlas. Pixel layout is makepad's
/// `VecBGRAu8_32` (`a<<24 | r<<16 | g<<8 | b`), i.e. what
/// `makepad_draw::image_cache::ImageBuffer` decodes to.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u32>,
}

/// Thin-lens camera. `fov_y` is the vertical field of view in radians;
/// `focal_mm`/`f_stop` size the aperture the way a photographer thinks
/// (`lens_radius = focal / (2 f) / 1000 * bokeh_scale` scene units) and
/// `focus_dist` is where the plane of sharp focus sits, in scene units.
#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub pos: Vec3f,
    pub target: Vec3f,
    pub up: Vec3f,
    pub fov_y: f32,
    pub focal_mm: f32,
    pub f_stop: f32,
    pub focus_dist: f32,
    /// Exaggerates the physical aperture (1.0 = a real lens; buildings
    /// photographed at 24mm f/2.8 show almost no bokeh, the money shot
    /// usually wants 4..10).
    pub bokeh_scale: f32,
    /// 0 = round aperture, 5..9 = a bladed iris (polygonal highlights).
    pub blades: u32,
    /// `Some(height)`: orthographic, `height` scene units tall (aspect from
    /// the target). `None`: perspective.
    pub ortho_height: Option<f32>,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            pos: vec3f(0.0, 1.5, 6.0),
            target: vec3f(0.0, 1.0, 0.0),
            up: vec3f(0.0, 1.0, 0.0),
            fov_y: 45.0f32.to_radians(),
            focal_mm: 35.0,
            f_stop: 2.8,
            focus_dist: 6.0,
            bokeh_scale: 1.0,
            blades: 0,
            ortho_height: None,
        }
    }
}

impl Camera {
    /// Aperture radius in scene units.
    pub fn lens_radius(&self) -> f32 {
        if self.f_stop <= 0.0 {
            return 0.0;
        }
        self.focal_mm / (2.0 * self.f_stop) / 1000.0 * self.bokeh_scale
    }
    /// Orthonormal basis: (right, up, forward).
    pub fn basis(&self) -> (Vec3f, Vec3f, Vec3f) {
        let fwd = (self.target - self.pos).normalize();
        let mut right = Vec3f::cross(fwd, self.up);
        if right.length() < 1.0e-6 {
            right = Vec3f::cross(fwd, vec3f(1.0, 0.0, 0.0));
        }
        let right = right.normalize();
        let up = Vec3f::cross(right, fwd).normalize();
        (right, up, fwd)
    }
}

/// The sun + analytic sky. `dir` points TOWARD the sun (unit, world space).
#[derive(Clone, Debug, PartialEq)]
pub struct Sun {
    pub dir: Vec3f,
    /// Preetham turbidity, 2 = crisp mountain air, 6 = hazy city.
    pub turbidity: f32,
    /// Multiplies the whole sky+sun (radiance scale into scene units).
    pub sky_strength: f32,
    /// Sun disc irradiance relative to the sky's; ~4 is a clear day.
    pub sun_strength: f32,
}

impl Default for Sun {
    fn default() -> Self {
        Self {
            dir: vec3f(0.4, 0.7, 0.3).normalize(),
            turbidity: 2.5,
            sky_strength: 1.0,
            sun_strength: 4.0,
        }
    }
}

impl Sun {
    /// Sun direction from a local time of day (hours) and latitude, with the
    /// sun rising in +X and `up` as the zenith. Azimuth sweeps X→Z (or X→−Y
    /// for a Z-up world) so the shadows walk across the plan over the day.
    pub fn from_time(hours: f32, latitude_deg: f32, up: Vec3f) -> Vec3f {
        let lat = latitude_deg.to_radians();
        let ha = (hours - 12.0) / 12.0 * std::f32::consts::PI; // hour angle
        // Summer-ish declination so the sun gets high at noon.
        let decl = 15.0f32.to_radians();
        let elev = (lat.sin() * decl.sin() + lat.cos() * decl.cos() * ha.cos()).asin();
        let az = ha; // 0 at noon (south), negative = morning (east)
        // Build in a y-up frame: east = +X, up = +Y, south = +Z.
        let d = vec3f(-az.sin() * elev.cos(), elev.sin(), az.cos() * elev.cos());
        // Re-express for the scene's up vector.
        if up.z.abs() > up.y.abs() {
            vec3f(d.x, -d.z, d.y).normalize()
        } else {
            d.normalize()
        }
    }
}

/// The flat snapshot. Triangle `i` is `indices[3i..3i+3]`, uses
/// `tri_material[i]`. `normals`/`uvs` are per vertex and optional (empty
/// means flat normals / no uvs).
#[derive(Clone, Debug, Default)]
pub struct SceneInput {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub tri_material: Vec<u32>,
    /// Render-time coplanar priority. Higher wins when two nearest hits are
    /// within the numeric/authoring tie window; zero disables prioritization.
    pub tri_priority: Vec<u16>,
    /// Measured overlap component. Priority is compared only for equal,
    /// non-zero groups, so unrelated nearby surfaces remain nearest-hit.
    pub tri_coplanar_group: Vec<u32>,
    pub materials: Vec<Material>,
    pub images: Vec<Image>,
    pub camera: Camera,
    pub sun: Sun,
    /// World up (unit) — sky dome and sun elevation reference.
    pub up: Vec3f,
}

impl SceneInput {
    pub fn tri_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> (Vec3f, Vec3f) {
        let mut lo = vec3f(f32::MAX, f32::MAX, f32::MAX);
        let mut hi = vec3f(f32::MIN, f32::MIN, f32::MIN);
        for p in &self.positions {
            lo = vec3f(lo.x.min(p[0]), lo.y.min(p[1]), lo.z.min(p[2]));
            hi = vec3f(hi.x.max(p[0]), hi.y.max(p[1]), hi.z.max(p[2]));
        }
        if self.positions.is_empty() {
            return (Vec3f::default(), Vec3f::default());
        }
        (lo, hi)
    }

    /// Append a mesh with one material; returns nothing, indices are rebased.
    pub fn push_mesh(
        &mut self,
        positions: &[[f32; 3]],
        normals: Option<&[[f32; 3]]>,
        uvs: Option<&[[f32; 2]]>,
        indices: &[u32],
        material: u32,
    ) {
        let base = self.positions.len() as u32;
        // Keep the per-vertex arrays either fully present or fully absent.
        if !self.positions.is_empty() && self.normals.is_empty() && normals.is_some() {
            self.normals = vec![[0.0; 3]; self.positions.len()];
        }
        if !self.positions.is_empty() && self.uvs.is_empty() && uvs.is_some() {
            self.uvs = vec![[0.0; 2]; self.positions.len()];
        }
        self.positions.extend_from_slice(positions);
        match normals {
            Some(n) => self.normals.extend_from_slice(n),
            None if !self.normals.is_empty() => {
                self.normals.extend(std::iter::repeat([0.0; 3]).take(positions.len()))
            }
            None => {}
        }
        match uvs {
            Some(u) => self.uvs.extend_from_slice(u),
            None if !self.uvs.is_empty() => {
                self.uvs.extend(std::iter::repeat([0.0; 2]).take(positions.len()))
            }
            None => {}
        }
        for tri in indices.chunks_exact(3) {
            self.indices.extend([tri[0] + base, tri[1] + base, tri[2] + base]);
            self.tri_material.push(material);
            self.tri_priority.push(0);
            self.tri_coplanar_group.push(0);
        }
    }

    /// A quad from four corners (counter-clockwise seen from the front).
    pub fn push_quad(&mut self, c: [[f32; 3]; 4], material: u32) {
        self.push_mesh(&c, None, None, &[0, 1, 2, 0, 2, 3], material);
    }

    /// Re-express a Y-up scene as Z-up (x, y, z) → (x, -z, y): positions,
    /// normals, camera and sun, `up` = +Z. A diagnostic for the Z-up path.
    pub fn to_z_up(&mut self) {
        let f = |v: [f32; 3]| [v[0], -v[2], v[1]];
        for p in self.positions.iter_mut() {
            *p = f(*p);
        }
        for n in self.normals.iter_mut() {
            *n = f(*n);
        }
        let fv = |v: Vec3f| vec3f(v.x, -v.z, v.y);
        self.camera.pos = fv(self.camera.pos);
        self.camera.target = fv(self.camera.target);
        self.camera.up = fv(self.camera.up);
        self.sun.dir = fv(self.sun.dir);
        self.up = vec3f(0.0, 0.0, 1.0);
    }

    /// Flat normals for every vertex that has none (zero length): the face
    /// normal, un-averaged, so a `SceneInput` built from quads shades right.
    pub fn ensure_normals(&mut self) {
        if self.normals.len() != self.positions.len() {
            self.normals = vec![[0.0; 3]; self.positions.len()];
        }
        let mut acc = vec![Vec3f::default(); self.positions.len()];
        for tri in self.indices.chunks_exact(3) {
            let p = |i: u32| {
                let v = self.positions[i as usize];
                vec3f(v[0], v[1], v[2])
            };
            let n = Vec3f::cross(p(tri[1]) - p(tri[0]), p(tri[2]) - p(tri[0]));
            for &i in tri {
                acc[i as usize] = acc[i as usize] + n;
            }
        }
        for (i, n) in self.normals.iter_mut().enumerate() {
            if n[0] == 0.0 && n[1] == 0.0 && n[2] == 0.0 {
                let a = acc[i];
                let l = a.length();
                if l > 0.0 {
                    *n = [a.x / l, a.y / l, a.z / l];
                }
            }
        }
    }

    /// The classic Cornell box (Y up, 2 units wide, camera at the open front),
    /// with an emissive ceiling panel. `glass_sphere` swaps the tall block
    /// for a thin-glass sphere so the window path gets exercised.
    pub fn cornell_box(glass_sphere: bool) -> SceneInput {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.materials = vec![
            Material::diffuse([0.73, 0.73, 0.73]), // 0 white
            Material::diffuse([0.65, 0.05, 0.05]), // 1 red
            Material::diffuse([0.12, 0.45, 0.15]), // 2 green
            Material::emissive([1.0, 0.85, 0.6], 18.0), // 3 light
            Material::glass([0.9, 0.95, 1.0]),     // 4 glass
        ];
        // floor, ceiling, back, left(red), right(green)
        s.push_quad([[-1.0, 0.0, -1.0], [-1.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0, 0.0, -1.0]], 0);
        s.push_quad([[-1.0, 2.0, -1.0], [1.0, 2.0, -1.0], [1.0, 2.0, 1.0], [-1.0, 2.0, 1.0]], 0);
        s.push_quad([[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 2.0, -1.0], [-1.0, 2.0, -1.0]], 0);
        s.push_quad([[-1.0, 0.0, 1.0], [-1.0, 0.0, -1.0], [-1.0, 2.0, -1.0], [-1.0, 2.0, 1.0]], 1);
        s.push_quad([[1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [1.0, 2.0, 1.0], [1.0, 2.0, -1.0]], 2);
        // light panel just under the ceiling
        let (l, y) = (0.3, 1.995);
        s.push_quad([[-l, y, -l], [l, y, -l], [l, y, l], [-l, y, l]], 3);
        // short block
        push_box(&mut s, vec3f(0.35, 0.0, 0.3), vec3f(0.5, 0.6, 0.5), -18.0f32.to_radians(), 0);
        if glass_sphere {
            push_sphere(&mut s, vec3f(-0.4, 0.45, -0.2), 0.45, 24, 4);
        } else {
            push_box(&mut s, vec3f(-0.4, 0.0, -0.3), vec3f(0.5, 1.2, 0.5), 17.0f32.to_radians(), 0);
        }
        s.ensure_normals();
        s.camera = Camera {
            pos: vec3f(0.0, 1.0, 3.9),
            target: vec3f(0.0, 1.0, 0.0),
            up: vec3f(0.0, 1.0, 0.0),
            fov_y: 39.0f32.to_radians(),
            focus_dist: 3.9,
            f_stop: 16.0,
            ..Default::default()
        };
        // No sun: the box is closed, the panel is the only light.
        s.sun = Sun { sky_strength: 0.0, sun_strength: 0.0, ..Default::default() };
        s
    }

    /// The furnace: a white diffuse sphere alone under a uniform white sky.
    /// Every pixel of the sphere must converge to exactly the sky radiance.
    pub fn furnace() -> SceneInput {
        let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
        s.materials = vec![Material::diffuse([1.0, 1.0, 1.0])];
        push_sphere(&mut s, vec3f(0.0, 0.0, 0.0), 1.0, 32, 0);
        s.ensure_normals();
        s.camera = Camera {
            pos: vec3f(0.0, 0.0, 4.0),
            target: vec3f(0.0, 0.0, 0.0),
            fov_y: 40.0f32.to_radians(),
            f_stop: 0.0,
            ..Default::default()
        };
        s
    }
}

/// Axis-aligned box rotated about Y around its base centre.
pub fn push_box(s: &mut SceneInput, base: Vec3f, size: Vec3f, rot_y: f32, material: u32) {
    let (c, sn) = (rot_y.cos(), rot_y.sin());
    let corner = |x: f32, y: f32, z: f32| {
        let (lx, lz) = ((x - 0.5) * size.x, (z - 0.5) * size.z);
        [base.x + lx * c - lz * sn, base.y + y * size.y, base.z + lx * sn + lz * c]
    };
    let faces: [[[f32; 3]; 4]; 6] = [
        [corner(0.0, 0.0, 1.0), corner(1.0, 0.0, 1.0), corner(1.0, 1.0, 1.0), corner(0.0, 1.0, 1.0)], // +z
        [corner(1.0, 0.0, 0.0), corner(0.0, 0.0, 0.0), corner(0.0, 1.0, 0.0), corner(1.0, 1.0, 0.0)], // -z
        [corner(1.0, 0.0, 1.0), corner(1.0, 0.0, 0.0), corner(1.0, 1.0, 0.0), corner(1.0, 1.0, 1.0)], // +x
        [corner(0.0, 0.0, 0.0), corner(0.0, 0.0, 1.0), corner(0.0, 1.0, 1.0), corner(0.0, 1.0, 0.0)], // -x
        [corner(0.0, 1.0, 1.0), corner(1.0, 1.0, 1.0), corner(1.0, 1.0, 0.0), corner(0.0, 1.0, 0.0)], // +y
        [corner(0.0, 0.0, 0.0), corner(1.0, 0.0, 0.0), corner(1.0, 0.0, 1.0), corner(0.0, 0.0, 1.0)], // -y
    ];
    for f in faces {
        s.push_quad(f, material);
    }
}

/// UV sphere with smooth normals.
pub fn push_sphere(s: &mut SceneInput, center: Vec3f, radius: f32, segs: u32, material: u32) {
    let rings = segs / 2;
    let mut pos = Vec::new();
    let mut nrm = Vec::new();
    let mut idx = Vec::new();
    for r in 0..=rings {
        let v = r as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for sg in 0..=segs {
            let u = sg as f32 / segs as f32;
            let phi = u * std::f32::consts::TAU;
            let n = vec3f(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin());
            nrm.push([n.x, n.y, n.z]);
            pos.push([center.x + n.x * radius, center.y + n.y * radius, center.z + n.z * radius]);
        }
    }
    let w = segs + 1;
    for r in 0..rings {
        for sg in 0..segs {
            let a = r * w + sg;
            let b = a + w;
            // Counter-clockwise from outside; geometric and smooth normals
            // agree so one-sided opaque spheres remain visible.
            idx.extend([a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    s.push_mesh(&pos, Some(&nrm), None, &idx, material);
}
