use crate::{formulas, m3p, render};
use makepad_zune_core::bit_depth::BitDepth;
use makepad_zune_core::colorspace::ColorSpace;
use makepad_zune_core::options::EncoderOptions;
use makepad_zune_png::PngEncoder;

#[path = "split_f32_vec2.rs"]
mod split_f32_vec2;

#[derive(Clone, Copy, Default, Debug)]
struct F2 {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Default, Debug)]
struct F3 {
    x: f32,
    y: f32,
    z: f32,
}

#[derive(Clone, Copy, Default, Debug)]
struct F4 {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
}

#[derive(Clone, Copy, Default, Debug)]
struct F3Split {
    x: F2,
    y: F2,
    z: F2,
}

impl F3 {
    fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn len(self) -> f32 {
        self.dot(self).sqrt()
    }

    fn normalize(self) -> Self {
        let len = self.len().max(1.0e-6);
        Self {
            x: self.x / len,
            y: self.y / len,
            z: self.z / len,
        }
    }

    fn scale(self, s: f32) -> Self {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }

    fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }

    fn mul_components(self, other: Self) -> Self {
        Self {
            x: self.x * other.x,
            y: self.y * other.y,
            z: self.z * other.z,
        }
    }
}

fn split_vec3(v: render::Vec3) -> F3Split {
    F3Split {
        x: split_f64(v.x),
        y: split_f64(v.y),
        z: split_f64(v.z),
    }
}

fn split_to_f64(v: F2) -> f64 {
    v.x as f64 + v.y as f64
}

fn split_vec3_to_vec3(v: F3Split) -> render::Vec3 {
    render::Vec3::new(split_to_f64(v.x), split_to_f64(v.y), split_to_f64(v.z))
}

fn mix_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix3(a: F3, b: F3, t: f32) -> F3 {
    F3 {
        x: mix_f32(a.x, b.x, t),
        y: mix_f32(a.y, b.y, t),
        z: mix_f32(a.z, b.z, t),
    }
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn rgb4(color: [u8; 3]) -> F4 {
    F4 {
        x: color[0] as f32 / 255.0,
        y: color[1] as f32 / 255.0,
        z: color[2] as f32 / 255.0,
        w: 1.0,
    }
}

fn split_f64(value: f64) -> F2 {
    let hi = value as f32;
    F2 {
        x: hi,
        y: (value - hi as f64) as f32,
    }
}

#[derive(Clone)]
struct AmazingUniforms {
    scale: f64,
    scale_div_min_r2: f64,
    min_r2: f64,
    fold: f64,
}

#[derive(Clone)]
struct MengerUniforms {
    scale: f64,
    cx: f64,
    cy: f64,
    cz: f64,
    rot: formulas::Mat3,
}

#[derive(Clone)]
struct CathedralScene {
    m3p: m3p::M3PFile,
    base_width: f64,
    formula0_iters: f32,
    formula1_iters: f32,
    repeat_from_slot: f32,
    amazing: AmazingUniforms,
    menger: MengerUniforms,
    light_dir: F3,
    light_color: F4,
    ambient_top: F4,
    ambient_bottom: F4,
    sky_color: F4,
    sky_color2: F4,
}

#[derive(Clone, Copy, Default, Debug)]
struct GpuDsUploads {
    cam_right: F3Split,
    cam_up: F3Split,
    cam_forward: F3Split,
    is_julia: bool,
    julia_x: F2,
    julia_y: F2,
    julia_z: F2,
    rot0: F3Split,
    rot1: F3Split,
    rot2: F3Split,
    mid_x: F2,
    mid_y: F2,
    mid_z: F2,
    fov_y: f32,
    step_width: F2,
    z_start_delta: F2,
    max_ray_length: F2,
    de_stop_header: F2,
    de_stop: F2,
    de_stop_factor: F2,
    s_z_step_div: F2,
    ms_de_sub: F2,
    mct_mh04_zsd: F2,
    s_z_step_div_raw: F2,
    de_floor: F2,
    de_scale: F2,
    bin_search_steps: usize,
    sm_normals: i32,
    first_step_random: bool,
    adaptive_ao_subsampling: bool,
    b_vary_de_stop: bool,
    d_fog_on_it: i32,
    hs_max_length_multiplier: F2,
    soft_shadow_radius: F2,
    rstop: F2,
    max_iters: i32,
    slot0_iters: i32,
    slot1_iters: i32,
    repeat_from_slot: usize,
    ab_scale: F2,
    ab_scale_div_min_r2: F2,
    ab_min_r2: F2,
    ab_fold: F2,
    menger_scale: F2,
    menger_cx: F2,
    menger_cy: F2,
    menger_cz: F2,
}

#[derive(Clone, Copy, Default, Debug)]
struct Ds {
    hi: f32,
    lo: f32,
}

impl Ds {
    const SPLITTER: f32 = 4097.0;

    fn new(v: f32) -> Self {
        Self { hi: v, lo: 0.0 }
    }

    fn from_split(v: F2) -> Self {
        Self { hi: v.x, lo: v.y }
    }

    fn quick_two_sum(a: f32, b: f32) -> Self {
        let s = a + b;
        let e = b - (s - a);
        Self { hi: s, lo: e }
    }

    fn two_sum(a: f32, b: f32) -> Self {
        let s = a + b;
        let bb = s - a;
        let e = (a - (s - bb)) + (b - bb);
        Self { hi: s, lo: e }
    }

    fn split(a: f32) -> (f32, f32) {
        let c = Self::SPLITTER * a;
        let hi = c - (c - a);
        let lo = a - hi;
        (hi, lo)
    }

    fn two_prod(a: f32, b: f32) -> Self {
        let p = a * b;
        let (a_hi, a_lo) = Self::split(a);
        let (b_hi, b_lo) = Self::split(b);
        let e = ((a_hi * b_hi - p) + a_hi * b_lo + a_lo * b_hi) + a_lo * b_lo;
        Self { hi: p, lo: e }
    }

    fn renorm(self) -> Self {
        Self::quick_two_sum(self.hi, self.lo)
    }

    fn add(self, other: Self) -> Self {
        let s = Self::two_sum(self.hi, other.hi);
        Self::quick_two_sum(s.hi, s.lo + self.lo + other.lo)
    }

    fn sub(self, other: Self) -> Self {
        self.add(Self {
            hi: -other.hi,
            lo: -other.lo,
        })
    }

    fn add_f(self, other: f32) -> Self {
        self.add(Self::new(other))
    }

    fn mul_f(self, other: f32) -> Self {
        let p = Self::two_prod(self.hi, other);
        Self::quick_two_sum(p.hi, p.lo + self.lo * other).renorm()
    }

    fn mul(self, other: Self) -> Self {
        let p = Self::two_prod(self.hi, other.hi);
        Self::quick_two_sum(
            p.hi,
            p.lo + self.hi * other.lo + self.lo * other.hi + self.lo * other.lo,
        )
        .renorm()
    }

    fn div(self, other: Self) -> Self {
        let q1 = self.hi / other.hi;
        let r = self.sub(other.mul_f(q1));
        let q2 = r.hi / other.hi;
        let r2 = r.sub(other.mul_f(q2));
        let q3 = r2.hi / other.hi;
        Self::quick_two_sum(q1, q2).add_f(q3).renorm()
    }

    fn abs(self) -> Self {
        if self.hi < 0.0 || (self.hi == 0.0 && self.lo < 0.0) {
            Self {
                hi: -self.hi,
                lo: -self.lo,
            }
        } else {
            self
        }
    }

    fn sqrt(self) -> Self {
        let x = self.to_f().max(0.0).sqrt();
        if x == 0.0 {
            return Self::new(0.0);
        }
        let xds = Self::new(x);
        xds.add(self.div(xds)).mul_f(0.5).renorm()
    }

    fn to_f(self) -> f32 {
        self.hi + self.lo
    }

    fn cmp(self, other: Self) -> std::cmp::Ordering {
        if self.hi < other.hi {
            std::cmp::Ordering::Less
        } else if self.hi > other.hi {
            std::cmp::Ordering::Greater
        } else if self.lo < other.lo {
            std::cmp::Ordering::Less
        } else if self.lo > other.lo {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    }
}

trait PortNum: Copy + Clone + std::fmt::Debug {
    fn zero() -> Self;
    fn one() -> Self;
    fn from_f64(v: f64) -> Self;
    fn from_split(v: F2) -> Self {
        Self::from_f64(v.x as f64 + v.y as f64)
    }
    fn to_f64(self) -> f64;
    fn add(self, other: Self) -> Self;
    fn sub(self, other: Self) -> Self;
    fn mul(self, other: Self) -> Self;
    fn div(self, other: Self) -> Self;
    fn abs(self) -> Self;
    fn sqrt(self) -> Self;
    fn cmp(self, other: Self) -> std::cmp::Ordering;

    fn add_f64(self, other: f64) -> Self {
        self.add(Self::from_f64(other))
    }

    fn mul_f64(self, other: f64) -> Self {
        self.mul(Self::from_f64(other))
    }

    fn lt_f64(self, other: f64) -> bool {
        self.cmp(Self::from_f64(other)) == std::cmp::Ordering::Less
    }
}

#[derive(Clone, Debug)]
struct OrbitScene<R: PortNum> {
    is_julia: bool,
    julia_x: R,
    julia_y: R,
    julia_z: R,
    rstop: R,
    max_iters: i32,
    slot0_iters: i32,
    slot1_iters: i32,
    repeat_from_slot: usize,
    ab_scale: R,
    ab_scale_div_min_r2: R,
    ab_min_r2: R,
    ab_fold: R,
    menger_scale: R,
    menger_cx: R,
    menger_cy: R,
    menger_cz: R,
    rot0: [R; 3],
    rot1: [R; 3],
    rot2: [R; 3],
}

#[derive(Clone, Copy, Debug)]
struct NumVec3<R: PortNum> {
    x: R,
    y: R,
    z: R,
}

impl<R: PortNum> NumVec3<R> {
    fn add(self, other: Self) -> Self {
        Self {
            x: self.x.add(other.x),
            y: self.y.add(other.y),
            z: self.z.add(other.z),
        }
    }

    fn scale(self, s: R) -> Self {
        Self {
            x: self.x.mul(s),
            y: self.y.mul(s),
            z: self.z.mul(s),
        }
    }

    fn dot(self, other: Self) -> R {
        self.x
            .mul(other.x)
            .add(self.y.mul(other.y))
            .add(self.z.mul(other.z))
    }

    fn normalize(self) -> Self {
        let len = self.dot(self).sqrt();
        if len.cmp(R::from_f64(1.0e-30)) == std::cmp::Ordering::Greater {
            Self {
                x: self.x.div(len),
                y: self.y.div(len),
                z: self.z.div(len),
            }
        } else {
            self
        }
    }
}

#[derive(Clone, Debug)]
struct MarchParams<R: PortNum> {
    step_width: R,
    max_ray_length: R,
    de_stop: R,
    de_stop_factor: R,
    s_z_step_div: R,
    ms_de_sub: R,
    mct_mh04_zsd: R,
    de_floor: R,
    max_iters: i32,
    bin_search_steps: usize,
    first_step_random: bool,
    d_fog_on_it: i32,
}

#[derive(Clone, Copy, Debug)]
enum MarchResult<R: PortNum> {
    Hit {
        depth: R,
        iters: i32,
        shadow_steps: i32,
    },
    Miss,
}

fn orbit_scene_from_ds_uploads<R: PortNum>(scene: &GpuDsUploads) -> OrbitScene<R> {
    OrbitScene {
        is_julia: scene.is_julia,
        julia_x: R::from_split(scene.julia_x),
        julia_y: R::from_split(scene.julia_y),
        julia_z: R::from_split(scene.julia_z),
        rstop: R::from_split(scene.rstop),
        max_iters: scene.max_iters,
        slot0_iters: scene.slot0_iters,
        slot1_iters: scene.slot1_iters,
        repeat_from_slot: scene.repeat_from_slot,
        ab_scale: R::from_split(scene.ab_scale),
        ab_scale_div_min_r2: R::from_split(scene.ab_scale_div_min_r2),
        ab_min_r2: R::from_split(scene.ab_min_r2),
        ab_fold: R::from_split(scene.ab_fold),
        menger_scale: R::from_split(scene.menger_scale),
        menger_cx: R::from_split(scene.menger_cx),
        menger_cy: R::from_split(scene.menger_cy),
        menger_cz: R::from_split(scene.menger_cz),
        rot0: [
            R::from_split(scene.rot0.x),
            R::from_split(scene.rot0.y),
            R::from_split(scene.rot0.z),
        ],
        rot1: [
            R::from_split(scene.rot1.x),
            R::from_split(scene.rot1.y),
            R::from_split(scene.rot1.z),
        ],
        rot2: [
            R::from_split(scene.rot2.x),
            R::from_split(scene.rot2.y),
            R::from_split(scene.rot2.z),
        ],
    }
}
impl PortNum for Ds {
    fn zero() -> Self { Self::new(0.0) }
    fn one() -> Self { Self::new(1.0) }
    fn from_f64(v: f64) -> Self {
        let hi = v as f32;
        Self {
            hi,
            lo: (v - hi as f64) as f32,
        }
    }
    fn from_split(v: F2) -> Self { Self::from_split(v) }
    fn to_f64(self) -> f64 { self.hi as f64 + self.lo as f64 }
    fn add(self, other: Self) -> Self { self.add(other) }
    fn sub(self, other: Self) -> Self { self.sub(other) }
    fn mul(self, other: Self) -> Self { self.mul(other) }
    fn div(self, other: Self) -> Self { self.div(other) }
    fn abs(self) -> Self { self.abs() }
    fn sqrt(self) -> Self { self.sqrt() }
    fn cmp(self, other: Self) -> std::cmp::Ordering { self.cmp(other) }
}

fn box_fold_num<R: PortNum>(a: R, fold: R) -> R {
    a.add(fold).abs()
        .sub(a.sub(fold).abs())
        .sub(a)
}

fn hybrid_de_scene<R: PortNum>(scene: &OrbitScene<R>, px: R, py: R, pz: R) -> (f32, R) {
    let cx = if scene.is_julia { scene.julia_x } else { px };
    let cy = if scene.is_julia { scene.julia_y } else { py };
    let cz = if scene.is_julia { scene.julia_z } else { pz };
    let mut x = px;
    let mut y = py;
    let mut z = pz;
    let mut w = R::one();
    let mut r2 = R::zero();
    let mut iters = 0.0f32;
    let mut slot = 0usize;
    let mut remaining = scene.slot0_iters;

    for _ in 0..128 {
        if remaining <= 0 {
            slot += 1;
            if slot >= 2 {
                slot = scene.repeat_from_slot;
            }
            remaining = if slot == 0 {
                scene.slot0_iters
            } else {
                scene.slot1_iters
            };
        }

        if slot == 0 {
            x = box_fold_num(x, scene.ab_fold);
            y = box_fold_num(y, scene.ab_fold);
            z = box_fold_num(z, scene.ab_fold);

            let rr = x.mul(x).add(y.mul(y)).add(z.mul(z));
            let m = if rr.cmp(scene.ab_min_r2) == std::cmp::Ordering::Less {
                scene.ab_scale_div_min_r2.to_f64()
            } else if rr.lt_f64(1.0) {
                scene.ab_scale.to_f64() / rr.to_f64().max(1.0e-7)
            } else {
                scene.ab_scale.to_f64()
            };

            w = w.mul_f64(m);
            x = x.mul_f64(m).add(cx);
            y = y.mul_f64(m).add(cy);
            z = z.mul_f64(m).add(cz);
        } else {
            x = x.abs();
            y = y.abs();
            z = z.abs();

            if x.cmp(y) == std::cmp::Ordering::Less {
                std::mem::swap(&mut x, &mut y);
            }
            if x.cmp(z) == std::cmp::Ordering::Less {
                std::mem::swap(&mut x, &mut z);
            }
            if y.cmp(z) == std::cmp::Ordering::Less {
                std::mem::swap(&mut y, &mut z);
            }

            let nx = x
                .mul(scene.rot0[0])
                .add(y.mul(scene.rot0[1]))
                .add(z.mul(scene.rot0[2]));
            let ny = x
                .mul(scene.rot1[0])
                .add(y.mul(scene.rot1[1]))
                .add(z.mul(scene.rot1[2]));
            let nz = x
                .mul(scene.rot2[0])
                .add(y.mul(scene.rot2[1]))
                .add(z.mul(scene.rot2[2]));

            let sf = scene.menger_scale.sub(R::one());
            x = nx
                .mul(scene.menger_scale)
                .sub(scene.menger_cx.mul(sf));
            y = ny
                .mul(scene.menger_scale)
                .sub(scene.menger_cy.mul(sf));

            let c = scene.menger_cz.mul(sf);
            z = c.sub(nz.mul(scene.menger_scale).sub(c).abs());
            w = w.mul(scene.menger_scale);
        }

        iters += 1.0;
        remaining -= 1;
        r2 = x.mul(x).add(y.mul(y)).add(z.mul(z));
        if r2.cmp(scene.rstop) == std::cmp::Ordering::Greater || iters >= scene.max_iters as f32 {
            break;
        }
    }

    let de = r2.sqrt().div(w.abs());
    (iters, de)
}
fn load_cathedral_scene(path: &str) -> Result<CathedralScene, String> {
    let m3p = m3p::parse(path).map_err(|err| format!("failed to parse {path}: {err}"))?;

    let end_to = (m3p.addon.b_hyb_opt1 & 7) as usize;
    let repeat_from = (m3p.addon.b_hyb_opt1 >> 4) as usize;
    let mut repeat_from_slot = None;
    let mut active = Vec::new();

    for i in 0..=end_to.min(5) {
        let formula = &m3p.addon.formulas[i];
        if formula.iteration_count <= 0 {
            continue;
        }
        if repeat_from_slot.is_none() && i >= repeat_from {
            repeat_from_slot = Some(active.len());
        }
        active.push(formula.clone());
    }

    if active.len() != 2 {
        return Err(format!(
            "split_f32 test expects exactly 2 active formulas, found {}",
            active.len()
        ));
    }

    let amazing_formula = &active[0];
    if amazing_formula.formula_nr != 4 {
        return Err(format!(
            "split_f32 test expects slot 0 AmazingBox, found #{} '{}'",
            amazing_formula.formula_nr, amazing_formula.custom_name
        ));
    }

    let amazing_min_r = amazing_formula.option_values[1].max(1.0e-40);
    let amazing_min_r2 = amazing_min_r * amazing_min_r;
    let amazing = AmazingUniforms {
        scale: amazing_formula.option_values[0],
        scale_div_min_r2: amazing_formula.option_values[0] / amazing_min_r2,
        min_r2: amazing_min_r2,
        fold: amazing_formula.option_values[2],
    };

    let menger_formula = &active[1];
    if !(menger_formula.custom_name.contains("Menger") || menger_formula.formula_nr == 20) {
        return Err(format!(
            "split_f32 test expects slot 1 MengerIFS, found #{} '{}'",
            menger_formula.formula_nr, menger_formula.custom_name
        ));
    }

    let rot_x = if menger_formula.option_count > 4 {
        menger_formula.option_values[4]
    } else {
        0.0
    };
    let rot_y = if menger_formula.option_count > 5 {
        menger_formula.option_values[5]
    } else {
        0.0
    };
    let rot_z = if menger_formula.option_count > 6 {
        menger_formula.option_values[6]
    } else {
        0.0
    };

    let rot = if rot_x == 0.0 && rot_y == 0.0 && rot_z == 0.0
    {
        formulas::Mat3::identity()
    } else {
        let d2r = std::f64::consts::PI / 180.0;
        formulas::Mat3::from_euler(
            rot_x * d2r,
            rot_y * d2r,
            rot_z * d2r,
        )
    };

    let menger_scale = if menger_formula.option_count > 0 {
        menger_formula.option_values[0]
    } else {
        3.0
    };
    let menger_cx = if menger_formula.option_count > 1 {
        menger_formula.option_values[1]
    } else {
        1.0
    };
    let menger_cy = if menger_formula.option_count > 2 {
        menger_formula.option_values[2]
    } else {
        1.0
    };
    let menger_cz = if menger_formula.option_count > 3 {
        menger_formula.option_values[3]
    } else {
        0.5
    };

    let menger = MengerUniforms {
        scale: menger_scale,
        cx: menger_cx,
        cy: menger_cy,
        cz: menger_cz,
        rot,
    };

    let camera = render::Camera::from_m3p(&m3p);
    let (light_dir, light_color) = select_primary_light(&m3p, &camera);

    Ok(CathedralScene {
        base_width: m3p.width as f64,
        formula0_iters: active[0].iteration_count as f32,
        formula1_iters: active[1].iteration_count as f32,
        repeat_from_slot: repeat_from_slot.unwrap_or(0) as f32,
        amazing,
        menger,
        light_dir,
        light_color,
        ambient_top: rgb4(m3p.lighting.ambient_top),
        ambient_bottom: rgb4(m3p.lighting.ambient_bottom),
        sky_color: rgb4(m3p.lighting.depth_col),
        sky_color2: rgb4(m3p.lighting.depth_col2),
        m3p,
    })
}

fn select_primary_light(m3p: &m3p::M3PFile, camera: &render::Camera) -> (F3, F4) {
    for light in &m3p.lighting.lights {
        let mut opt = (light.l_option & 3) as i32;
        if opt == 3 {
            opt = 1;
        }
        if opt != 0 || light.l_amp <= 0.0 {
            continue;
        }

        let local_dir = render::Vec3::new(
            -light.angle_xy.sin(),
            -light.angle_z.sin(),
            -(light.angle_xy.cos() * light.angle_z.cos()),
        )
        .normalize();
        let r = camera.right.normalize();
        let u = camera.up.normalize();
        let f = camera.forward.normalize();
        let dir = r
            .scale(local_dir.x)
            .add(u.scale(local_dir.y))
            .add(f.scale(local_dir.z))
            .normalize();

        let lamp_mul = if ((light.l_option >> 2) & 1) != 0 {
            light.l_amp * 1.3
        } else {
            light.l_amp
        } as f32;

        return (
            F3 {
                x: dir.x as f32,
                y: dir.y as f32,
                z: dir.z as f32,
            },
            F4 {
                x: (light.color[0] as f32 / 255.0) * lamp_mul,
                y: (light.color[1] as f32 / 255.0) * lamp_mul,
                z: (light.color[2] as f32 / 255.0) * lamp_mul,
                w: 1.0,
            },
        );
    }

    (
        F3 {
            x: -0.35,
            y: 0.8,
            z: 0.45,
        },
        F4 {
            x: 0.85,
            y: 0.82,
            z: 0.78,
            w: 1.0,
        },
    )
}

fn build_ds_uploads(scene: &CathedralScene, width: usize) -> GpuDsUploads {
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);

    GpuDsUploads {
        cam_right: split_vec3(params.camera.right.normalize()),
        cam_up: split_vec3(params.camera.up.normalize()),
        cam_forward: split_vec3(params.camera.forward.normalize()),
        is_julia: params.iter_params.is_julia,
        julia_x: split_f64(params.iter_params.julia_x),
        julia_y: split_f64(params.iter_params.julia_y),
        julia_z: split_f64(params.iter_params.julia_z),
        rot0: F3Split {
            x: split_f64(scene.menger.rot.m[0][0]),
            y: split_f64(scene.menger.rot.m[0][1]),
            z: split_f64(scene.menger.rot.m[0][2]),
        },
        rot1: F3Split {
            x: split_f64(scene.menger.rot.m[1][0]),
            y: split_f64(scene.menger.rot.m[1][1]),
            z: split_f64(scene.menger.rot.m[1][2]),
        },
        rot2: F3Split {
            x: split_f64(scene.menger.rot.m[2][0]),
            y: split_f64(scene.menger.rot.m[2][1]),
            z: split_f64(scene.menger.rot.m[2][2]),
        },
        mid_x: split_f64(params.camera.mid.x),
        mid_y: split_f64(params.camera.mid.y),
        mid_z: split_f64(params.camera.mid.z),
        fov_y: params.camera.fov_y as f32,
        step_width: split_f64(params.step_width),
        z_start_delta: split_f64(params.camera.z_start - params.camera.mid.z),
        max_ray_length: split_f64(params.max_ray_length),
        de_stop_header: split_f64(params.de_stop_header),
        de_stop: split_f64(params.de_stop),
        de_stop_factor: split_f64(params.de_stop_factor),
        s_z_step_div: split_f64(params.s_z_step_div),
        ms_de_sub: split_f64(params.ms_de_sub),
        mct_mh04_zsd: split_f64(params.mct_mh04_zsd),
        s_z_step_div_raw: split_f64(params.s_z_step_div_raw),
        de_floor: split_f64(params.de_floor),
        de_scale: split_f64(params.de_scale),
        bin_search_steps: params.bin_search_steps as usize,
        sm_normals: params.sm_normals,
        first_step_random: params.first_step_random,
        adaptive_ao_subsampling: params.adaptive_ao_subsampling,
        b_vary_de_stop: params.b_vary_de_stop,
        d_fog_on_it: params.d_fog_on_it as i32,
        hs_max_length_multiplier: split_f64(params.hs_max_length_multiplier),
        soft_shadow_radius: split_f64(params.soft_shadow_radius),
        rstop: split_f64(params.iter_params.rstop),
        max_iters: params.iter_params.max_iters,
        slot0_iters: scene.formula0_iters as i32,
        slot1_iters: scene.formula1_iters as i32,
        repeat_from_slot: scene.repeat_from_slot as usize,
        ab_scale: split_f64(scene.amazing.scale),
        ab_scale_div_min_r2: split_f64(scene.amazing.scale_div_min_r2),
        ab_min_r2: split_f64(scene.amazing.min_r2),
        ab_fold: split_f64(scene.amazing.fold),
        menger_scale: split_f64(scene.menger.scale),
        menger_cx: split_f64(scene.menger.cx),
        menger_cy: split_f64(scene.menger.cy),
        menger_cz: split_f64(scene.menger.cz),
    }
}

#[derive(Clone, Copy)]
struct StandaloneParsedLight {
    idx: usize,
    dir: render::Vec3,
    color: render::Vec3,
    spec_power: f64,
    diff_mode: i32,
    l_option: u8,
    i_light_pos: u8,
    is_positional: bool,
}

#[derive(Clone, Copy)]
struct StandaloneColorStop {
    pos: f64,
    color: render::Vec3,
}

struct StandaloneLightingState {
    parsed_lights: Vec<StandaloneParsedLight>,
    surface_diff_stops: Vec<StandaloneColorStop>,
    surface_spec_stops: Vec<StandaloneColorStop>,
    amb_bottom: render::Vec3,
    amb_top: render::Vec3,
    depth_col: render::Vec3,
    depth_col2: render::Vec3,
    dyn_fog_col: render::Vec3,
    dyn_fog_col2: render::Vec3,
    cam_up: render::Vec3,
    s_depth: f64,
    tbpos_3: i32,
    tbpos_6: i32,
    calc_pix_col_sqr: bool,
    rough_scale: f64,
    s_c_start: f64,
    s_c_mul: f64,
    s_col_z_mul: f64,
    s_diff: f64,
    s_spec: f64,
    z1: f64,
    depth_range: f64,
    b_amb_rel_obj: bool,
    i_dfunc: i32,
    b_dfog_options: u8,
}

struct StandaloneCosTables {
    diff_small: [[f64; 128]; 8],
}

fn standalone_light_local_dir(angle_xy: f64, angle_z: f64) -> render::Vec3 {
    render::Vec3::new(-angle_xy.sin(), -angle_z.sin(), -(angle_xy.cos() * angle_z.cos())).normalize()
}

fn standalone_light_is_active_non_lightmap(l: &m3p::M3PLight) -> bool {
    let mut opt = (l.l_option & 3) as i32;
    if opt == 3 {
        opt = 1;
    }
    opt == 0
}

fn parse_standalone_light(
    idx: usize,
    l: &m3p::M3PLight,
    camera: &render::Camera,
) -> Option<StandaloneParsedLight> {
    if !standalone_light_is_active_non_lightmap(l) {
        return None;
    }
    let lamp = l.l_amp.max(0.0);
    if lamp <= 0.0 {
        return None;
    }

    let i_light_pos = ((l.l_option >> 2) & 7) | ((l.l_function & 0x80) >> 4);
    let is_positional = (i_light_pos & 1) != 0;
    let diff_mode = ((l.l_function >> 4) & 3) as i32;
    let spec_power = (2u32 << (l.l_function & 0x07)) as f64;

    let local_dir = standalone_light_local_dir(l.angle_xy, l.angle_z);
    let r = camera.right.normalize();
    let u = camera.up.normalize();
    let f = camera.forward.normalize();
    let dir = r
        .scale(local_dir.x)
        .add(u.scale(local_dir.y))
        .add(f.scale(local_dir.z))
        .normalize();

    let mut color = render::Vec3::new(
        l.color[0] as f64 / 255.0,
        l.color[1] as f64 / 255.0,
        l.color[2] as f64 / 255.0,
    );
    let lamp_mul = if is_positional { lamp * 1.3 } else { lamp };
    color = color.scale(lamp_mul);

    if color.x <= 0.0 && color.y <= 0.0 && color.z <= 0.0 {
        return None;
    }

    Some(StandaloneParsedLight {
        idx,
        dir,
        color,
        spec_power,
        diff_mode,
        l_option: l.l_option,
        i_light_pos,
        is_positional,
    })
}

fn build_standalone_color_stops(lighting: &m3p::M3PLighting, specular: bool) -> Vec<StandaloneColorStop> {
    let mut stops = Vec::with_capacity(lighting.l_cols.len() + 1);
    for stop in &lighting.l_cols {
        let color = if specular {
            render::Vec3::new(
                stop.color_spe[0] as f64 / 255.0,
                stop.color_spe[1] as f64 / 255.0,
                stop.color_spe[2] as f64 / 255.0,
            )
        } else {
            render::Vec3::new(
                stop.color_dif[0] as f64 / 255.0,
                stop.color_dif[1] as f64 / 255.0,
                stop.color_dif[2] as f64 / 255.0,
            )
        };
        stops.push(StandaloneColorStop {
            pos: stop.pos as f64 / 32768.0,
            color,
        });
    }
    stops.sort_by(|a, b| a.pos.partial_cmp(&b.pos).unwrap());
    if let Some(first) = stops.first().copied() {
        stops.push(StandaloneColorStop {
            pos: 1.0 + first.pos,
            color: first.color,
        });
    }
    stops
}

fn standalone_sample_color_cycle(
    si_gradient: i32,
    stops: &[StandaloneColorStop],
    default: render::Vec3,
) -> render::Vec3 {
    if stops.is_empty() {
        return default;
    }

    let mut t = si_gradient as f64 / 32768.0;
    t -= t.floor();
    if t < 0.0 {
        t += 1.0;
    }

    let mut color = stops.last().unwrap().color;
    for i in 0..stops.len() - 1 {
        let first = stops[i];
        let second = stops[i + 1];
        let mut t_check = t;
        if i == stops.len() - 2 && t < first.pos {
            t_check += 1.0;
        }

        if t_check >= first.pos && t_check <= second.pos {
            let f = if second.pos > first.pos {
                (t_check - first.pos) / (second.pos - first.pos)
            } else {
                0.0
            };
            color = render::Vec3::new(
                first.color.x * (1.0 - f) + second.color.x * f,
                first.color.y * (1.0 - f) + second.color.y * f,
                first.color.z * (1.0 - f) + second.color.z * f,
            );
            break;
        }
    }

    color
}

fn standalone_cos_tables() -> &'static StandaloneCosTables {
    static TABLES: std::sync::OnceLock<StandaloneCosTables> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let mut diff_small = [[0.0f64; 128]; 8];

        for i in 0..128 {
            let d = 1.0 - (i as f64 - 2.0) / 60.0;
            diff_small[0][i] = if d > 0.15 {
                (d - 0.08) * 1.086_956_5
            } else if d <= 0.0 {
                0.0
            } else {
                d.powf(((0.505 - d) * 3.8).max(1.0))
            };
            diff_small[1][i] = d.max(0.0) * d.max(0.0);
            diff_small[2][i] = d * 0.5 + 0.5;
            diff_small[3][i] = diff_small[2][i] * diff_small[2][i];
        }

        for k in 0..4 {
            let mut tmp = [0.0f64; 128];
            for j in 0..128 {
                tmp[j] = diff_small[k][j].max(0.0).sqrt();
            }
            for j in 0..128 {
                let mut e = 0.0;
                for i in 0..=60 {
                    let l = (j as i32 + i - 30).abs() as usize;
                    if l < 128 {
                        e += tmp[l];
                    }
                }
                let t = e * 0.011 + (e * 0.007) * (e * 0.007);
                diff_small[k + 4][j] = t * t;
            }
        }

        StandaloneCosTables { diff_small }
    })
}

fn standalone_make_spline_coeff(xs: f64) -> [f64; 4] {
    let w3 = (1.0 / 6.0) * xs * xs * xs;
    let w0 = (1.0 / 6.0) + 0.5 * xs * (xs - 1.0) - w3;
    let w2 = xs + w0 - 2.0 * w3;
    let w1 = 1.0 - w0 - w2 - w3;
    [w0, w1, w2, w3]
}

fn standalone_interp_tab4(tab: &[f64; 128], ip: usize, w: [f64; 4]) -> f64 {
    tab[ip] * w[0] + tab[ip + 1] * w[1] + tab[ip + 2] * w[2] + tab[ip + 3] * w[3]
}

fn standalone_get_cos_tab_val_inner(tnr: i32, dotp: f64, rough: f64, square: bool) -> f64 {
    let tnr = tnr.clamp(0, 3) as usize;
    let rough = rough.clamp(0.0, 1.0);
    let mut t = 62.0 - 60.0 * dotp;
    let mut ip = t.trunc() as i32 - 1;
    if ip < 0 {
        ip = 0;
        t = 0.0;
    } else if ip > 124 {
        ip = 124;
        t = 1.0;
    } else {
        t = t.fract();
    }
    let ipu = ip as usize;
    let w = standalone_make_spline_coeff(t);
    let tables = standalone_cos_tables();
    let a = standalone_interp_tab4(&tables.diff_small[tnr], ipu, w);
    let b = standalone_interp_tab4(&tables.diff_small[tnr + 4], ipu, w);
    if square {
        let a2 = a * a;
        let b2 = b * b;
        a2 + rough * (b2 - a2)
    } else {
        a + rough * (b - a)
    }
}

fn standalone_apply_diff_mode_mb3d(mode: i32, ndotl: f64, rough: f64, calc_pix_col_sqr: bool) -> f64 {
    standalone_get_cos_tab_val_inner(mode, ndotl, rough, calc_pix_col_sqr)
}

fn standalone_soft_hs_light_dir(
    lighting: &m3p::M3PLighting,
    camera: &render::Camera,
    params: &render::RenderParams,
) -> Option<(usize, render::Vec3, u8)> {
    if (params.b_calc1_hs_soft & 1) == 0 {
        return None;
    }

    let mut selected = None;
    let hs_bits = params.b_calculate_hard_shadow as u16;
    for itmp in 0..6usize {
        let mask = 4u16 << itmp;
        if (hs_bits & mask) != 0 {
            selected = Some(itmp);
        }
    }
    let idx = selected?;
    let l = lighting.lights.get(idx)?;
    let pl = parse_standalone_light(idx, l, camera)?;
    if pl.is_positional {
        return None;
    }
    Some((idx, pl.dir, pl.i_light_pos))
}

fn build_standalone_lighting_state(
    lighting: &m3p::M3PLighting,
    camera: &render::Camera,
    params: &render::RenderParams,
) -> StandaloneLightingState {
    let mut parsed_lights = Vec::with_capacity(lighting.lights.len());
    for (idx, light) in lighting.lights.iter().enumerate() {
        if let Some(parsed) = parse_standalone_light(idx, light, camera) {
            parsed_lights.push(parsed);
        }
    }

    let mut s_c_start =
        ((lighting.tbpos_9 + 30) as f64 * 0.01111111111111111).powi(2) * 32767.0 - 10900.0;
    let mut s_c_mul =
        ((lighting.tbpos_10 + 30) as f64 * 0.01111111111111111).powi(2) * 32767.0 - 10900.0
            - s_c_start;
    if (lighting.tboptions & 0x10000) != 0 {
        let adjusted =
            s_c_start + s_c_mul * (lighting.fine_col_adj_2 as i32 - 30) as f64 * 0.0166666666666666;
        s_c_start += s_c_mul * (lighting.fine_col_adj_1 as i32 - 30) as f64 * 0.0166666666666666;
        s_c_mul = adjusted - s_c_start;
    }
    if s_c_mul.abs() > 1e-4 {
        s_c_mul = 2.0 / s_c_mul;
    } else if s_c_mul < 0.0 {
        s_c_mul = -2000.0;
    } else {
        s_c_mul = 2000.0;
    }

    let s_col_z_mul = if (lighting.tboptions & 0x20000) != 0 {
        (lighting.tbpos_11 as f64 * -0.005) / (params.step_width * 1920.0)
    } else {
        0.0
    };

    let mut b_dfog_options = lighting
        .lights
        .first()
        .map(|light| light.free_byte & 3)
        .unwrap_or(0);
    if b_dfog_options == 3 {
        b_dfog_options = 1;
    }

    StandaloneLightingState {
        parsed_lights,
        surface_diff_stops: build_standalone_color_stops(lighting, false),
        surface_spec_stops: build_standalone_color_stops(lighting, true),
        amb_bottom: render::Vec3::new(
            lighting.ambient_bottom[0] as f64 / 255.0,
            lighting.ambient_bottom[1] as f64 / 255.0,
            lighting.ambient_bottom[2] as f64 / 255.0,
        ),
        amb_top: render::Vec3::new(
            lighting.ambient_top[0] as f64 / 255.0,
            lighting.ambient_top[1] as f64 / 255.0,
            lighting.ambient_top[2] as f64 / 255.0,
        ),
        depth_col: render::Vec3::new(
            lighting.depth_col[0] as f64 / 255.0,
            lighting.depth_col[1] as f64 / 255.0,
            lighting.depth_col[2] as f64 / 255.0,
        ),
        depth_col2: render::Vec3::new(
            lighting.depth_col2[0] as f64 / 255.0,
            lighting.depth_col2[1] as f64 / 255.0,
            lighting.depth_col2[2] as f64 / 255.0,
        ),
        dyn_fog_col: render::Vec3::new(
            lighting.dyn_fog_col[0] as f64 / 255.0,
            lighting.dyn_fog_col[1] as f64 / 255.0,
            lighting.dyn_fog_col[2] as f64 / 255.0,
        ),
        dyn_fog_col2: render::Vec3::new(
            lighting.dyn_fog_col2[0] as f64 / 255.0,
            lighting.dyn_fog_col2[1] as f64 / 255.0,
            lighting.dyn_fog_col2[2] as f64 / 255.0,
        ),
        cam_up: camera.up.normalize(),
        s_depth: lighting.s_depth,
        tbpos_3: lighting.tbpos_3,
        tbpos_6: lighting.tbpos_6,
        calc_pix_col_sqr: lighting.calc_pix_col_sqr,
        rough_scale: lighting.roughness_factor as f64 / (255.0 * 255.0),
        s_c_start,
        s_c_mul,
        s_col_z_mul,
        s_diff: (lighting.tbpos_5 as f64 * 0.02).max(0.0),
        s_spec: (((lighting.tbpos_7 & 0x0FFF) as f64) * 0.02).max(0.004),
        z1: camera.z_start - camera.mid.z,
        depth_range: camera.z_end - camera.z_start,
        b_amb_rel_obj: (lighting.tboptions & 0x20000000) != 0,
        i_dfunc: ((lighting.tboptions >> 30) & 0x3) as i32,
        b_dfog_options,
    }
}

fn standalone_shade_with_final_ao_mb3d(
    state: &StandaloneLightingState,
    ssao: &m3p::M3PSSAO,
    params: &render::RenderParams,
    normal: render::Vec3,
    roughness: f64,
    view_dir: render::Vec3,
    iters: i32,
    shadow_steps: i32,
    final_ao: f64,
    depth: f64,
    y_pos: f64,
    max_depth: f64,
) -> [u8; 3] {
    let m_zz = depth / params.step_width;
    let diffuse_shadowing = ssao.diffuse_shadowing.clamp(0.0, 1.0);
    let diff_ao = (1.0 - diffuse_shadowing) + diffuse_shadowing * final_ao;
    let rough_byte = (roughness.clamp(0.0, 1.0) * 255.0).round();
    let d_rough = rough_byte * state.rough_scale;

    let d_tmp = iters as f64;
    let max_it = params.iter_params.max_iters as f64;
    let min_it = params.iter_params.min_iters as f64;
    let mut si_gradient_f = 32767.0 - (d_tmp - min_it) * 32767.0 / (max_it - min_it + 1.0);
    if si_gradient_f > 32766.5 {
        si_gradient_f = 32767.0;
    }
    if si_gradient_f < 0.0 {
        si_gradient_f = 0.0;
    }
    let si_gradient = si_gradient_f.round() as i32;

    let plv_z_pos = depth + state.z1;
    let i_dif_0 = state.s_col_z_mul * plv_z_pos;
    let ir_f = ((si_gradient as f64 - state.s_c_start) * state.s_c_mul + i_dif_0) * 16384.0;
    let ir_cycled = (ir_f.round() as i32) & 32767;

    let diffuse_color = standalone_sample_color_cycle(
        ir_cycled,
        &state.surface_diff_stops,
        render::Vec3::new(0.5, 0.5, 0.5),
    );
    let spec_color = standalone_sample_color_cycle(
        ir_cycled,
        &state.surface_spec_stops,
        render::Vec3::new(1.0, 1.0, 1.0),
    );

    let v_from_cam = view_dir.normalize().scale(-1.0);
    let n = normal.normalize();
    let ny = if state.b_amb_rel_obj { n.y } else { n.dot(state.cam_up) };
    let w_top = (ny * 0.5 + 0.5).clamp(0.0, 1.0);
    let w_bot = 1.0 - w_top;
    let amb_light = state
        .amb_top
        .scale(w_top)
        .add(state.amb_bottom.scale(w_bot))
        .scale(final_ao);

    let mut total_diffuse = render::Vec3::new(0.0, 0.0, 0.0);
    let mut total_specular = render::Vec3::new(0.0, 0.0, 0.0);
    for pl in &state.parsed_lights {
        let li = pl.idx;
        let i_hs_enabled = 1 - (((pl.l_option >> 6) & 1) as i32);
        let i_hs_calced = i_hs_enabled & (((params.b_hs_calculated as i32) >> (li + 2)) & 1);
        let mut i_hs_mask = 0x400i32 << li;
        if ((params.b_calc1_hs_soft & 1) != 0) && (i_hs_calced != 0) {
            i_hs_mask = -1;
        }
        let soft_hs = i_hs_mask == -1;
        let no_hs = soft_hs || ((shadow_steps & i_hs_mask) == 0) || (i_hs_calced == 0);
        let b_sub_amb_sh = (i_hs_calced ^ i_hs_enabled) != 0;
        let mut hs_mul = if b_sub_amb_sh { final_ao } else { diff_ao };
        if soft_hs {
            let soft = ((shadow_steps >> 10) as f64 * (1.0 / 63.0)).clamp(0.0, 1.0);
            hs_mul *= soft;
        }
        let light_gate = if no_hs { hs_mul } else { 0.0 };

        let diff_dot = standalone_apply_diff_mode_mb3d(
            pl.diff_mode,
            n.dot(pl.dir),
            d_rough,
            state.calc_pix_col_sqr,
        );
        total_diffuse = total_diffuse.add(pl.color.scale(diff_dot * light_gate));

        let reflect_view = v_from_cam.sub(n.scale(2.0 * n.dot(v_from_cam)));
        let spec_dot = pl.dir.dot(reflect_view);
        if spec_dot > 0.0 {
            let att = 1.0;
            let mut spec_mul =
                (att + (d_rough * 2.0).min(1.0) * (1.0 / pl.spec_power - att)) * state.s_spec;
            if spec_mul < 0.0 {
                spec_mul = 0.0;
            }
            if spec_mul > 0.0 {
                let spec_pow = spec_dot.powf(pl.spec_power);
                total_specular = total_specular.add(pl.color.scale(spec_pow * spec_mul * light_gate));
            }
        }
    }

    let s = if state.b_amb_rel_obj {
        (v_from_cam.y.asin() / std::f64::consts::PI + 0.5).clamp(0.0, 1.0)
    } else {
        let yy = y_pos.clamp(0.0, 1.0);
        match state.i_dfunc {
            1 => yy * yy,
            0 => yy,
            _ => yy.sqrt(),
        }
    };
    let dep_c_interp = render::Vec3::new(
        state.depth_col2.x * s + state.depth_col.x * (1.0 - s),
        state.depth_col2.y * s + state.depth_col.y * (1.0 - s),
        state.depth_col2.z * s + state.depth_col.z * (1.0 - s),
    );

    let mut final_color = render::Vec3::new(
        amb_light.x * diffuse_color.x + diffuse_color.x * state.s_diff * total_diffuse.x
            + spec_color.x * total_specular.x,
        amb_light.y * diffuse_color.y + diffuse_color.y * state.s_diff * total_diffuse.y
            + spec_color.y * total_specular.y,
        amb_light.z * diffuse_color.z + diffuse_color.z * state.s_diff * total_diffuse.z
            + spec_color.z * total_specular.z,
    );

    let z_pos_f = 32767.0 - (params.z_cmul / 256.0) * ((m_zz * params.z_corr + 1.0).sqrt() - 1.0);
    let mut z_pos = z_pos_f.round().clamp(0.0, 32767.0) as i32;
    if depth >= max_depth * 0.9999 {
        z_pos = 32768;
    }
    if z_pos >= 32768 {
        final_color = dep_c_interp;
    }

    let d_tmp = if z_pos < 32768 {
        (1.0 + (z_pos_f - 28000.0) * state.s_depth).max(0.0)
    } else {
        (1.0 - (60768.0 - z_pos_f) * state.s_depth).max(0.0)
    };

    let mut s_tmp_shad = 128.0;
    let b_vol_light = (params.b_vol_light_nr & 7) != 0;
    let mut d_tmp_shad = 2.2 / params.s_z_step_div_raw;
    let mut s_shad_gr = (state.tbpos_6 as f64 - 53.0) * params.s_z_step_div_raw * 0.00065;
    let mut s_dyn_fog_mul = params.s_z_step_div_raw * 0.015;
    if b_vol_light {
        s_dyn_fog_mul = 0.0005;
        d_tmp_shad = 50.0;
        s_shad_gr = (state.tbpos_6 as f64 - 53.0) * 0.00002;
    } else if params.b_dfog_it > 0 {
        d_tmp_shad *= 0.25;
        s_shad_gr *= 4.0;
        s_dyn_fog_mul *= 4.0;
    } else {
        s_tmp_shad = 137.0;
    }

    let sqrt_tbpos3_and_ffff = ((state.tbpos_3 & 0xFFFF) as f64).sqrt();
    let s_shad = (s_tmp_shad - sqrt_tbpos3_and_ffff * 11.313708) * d_tmp_shad * 0.28;
    let sqrt_tbpos3_shr_16 = ((state.tbpos_3 >> 16) as f64).sqrt();
    let s_shad_z_mul =
        d_tmp_shad * 0.7 / state.depth_range * (128.0 - sqrt_tbpos3_shr_16 * 11.313708);

    let ir_for_fog = if b_vol_light {
        let mut eax = shadow_steps & 0x3FF;
        let cl = eax >> 7;
        eax &= 0x7F;
        eax <<= cl;
        eax as f64
    } else {
        (shadow_steps & 0x3FF) as f64
    };

    let mut d_fog = (ir_for_fog - s_shad - s_shad_z_mul * plv_z_pos) * s_shad_gr;
    if (state.b_dfog_options & 2) != 0 {
        d_fog = d_fog.max(0.0);
    }

    let mut d_tmp3 = (1.0f64).min(ir_for_fog * s_dyn_fog_mul) * d_fog;
    if (state.b_dfog_options & 1) != 0 {
        d_fog = d_fog.clamp(0.0, 1.0);
        d_tmp3 = d_tmp3.clamp(0.0, 1.0);
    }

    let fog_add = render::Vec3::new(
        state.dyn_fog_col.x * (d_fog - d_tmp3) + state.dyn_fog_col2.x * d_tmp3,
        state.dyn_fog_col.y * (d_fog - d_tmp3) + state.dyn_fog_col2.y * d_tmp3,
        state.dyn_fog_col.z * (d_fog - d_tmp3) + state.dyn_fog_col2.z * d_tmp3,
    );

    let t_dep = (1.0f64 - d_tmp).max(0.0f64);
    final_color = render::Vec3::new(
        final_color.x * d_tmp + dep_c_interp.x * t_dep,
        final_color.y * d_tmp + dep_c_interp.y * t_dep,
        final_color.z * d_tmp + dep_c_interp.z * t_dep,
    );
    if (state.b_dfog_options & 1) != 0 {
        final_color = render::Vec3::new(
            final_color.x * (1.0 - d_fog),
            final_color.y * (1.0 - d_fog),
            final_color.z * (1.0 - d_fog),
        );
    }
    final_color = render::Vec3::new(
        final_color.x + fog_add.x,
        final_color.y + fog_add.y,
        final_color.z + fog_add.z,
    );

    let toned = render::Vec3::new(
        final_color.x.clamp(0.0, 1.0),
        final_color.y.clamp(0.0, 1.0),
        final_color.z.clamp(0.0, 1.0),
    );
    [
        (toned.x * 255.0) as u8,
        (toned.y * 255.0) as u8,
        (toned.z * 255.0) as u8,
    ]
}

fn encode_png(path: &str, pixels: &[u8], width: usize, height: usize) -> Result<(), String> {
    let options = EncoderOptions::default()
        .set_width(width)
        .set_height(height)
        .set_depth(BitDepth::Eight)
        .set_colorspace(ColorSpace::RGBA);
    let mut encoder = PngEncoder::new(pixels, options);
    let mut out = Vec::new();
    encoder
        .encode(&mut out)
        .map_err(|err| format!("png encode failed: {err:?}"))?;
    std::fs::write(path, &out).map_err(|err| format!("write failed: {err}"))?;
    Ok(())
}

fn shaderlike_ray_for_pixel_num<R: PortNum>(
    scene: &GpuDsUploads,
    width: usize,
    height: usize,
    x: usize,
    y: usize,
) -> (NumVec3<R>, NumVec3<R>) {
    let frag_x = x as f32;
    let frag_y = y as f32;
    let half_w = width as f32 * 0.5;
    let half_h = height as f32 * 0.5;
    let fov_mul = (scene.fov_y * 0.017453292519943295_f32) / height.max(1) as f32;

    let cafx = (half_w - frag_x) * fov_mul;
    let cafy = (frag_y - half_h) * fov_mul;
    let sx = cafx.sin();
    let cx = cafx.cos();
    let sy = cafy.sin();
    let cy = cafy.cos();

    let local_dir = F3 {
        x: -sx,
        y: sy,
        z: cx * cy,
    }
    .normalize();

    let cam_right = split_vec3_to_num::<R>(scene.cam_right);
    let cam_up = split_vec3_to_num::<R>(scene.cam_up);
    let cam_forward = split_vec3_to_num::<R>(scene.cam_forward);

    let dir = cam_right
        .scale(R::from_f64(local_dir.x as f64))
        .add(cam_up.scale(R::from_f64(local_dir.y as f64)))
        .add(cam_forward.scale(R::from_f64(local_dir.z as f64)))
        .normalize();

    let step_width = R::from_split(scene.step_width);
    let x_offset = R::from_f64((frag_x - half_w) as f64).mul(step_width);
    let y_offset = R::from_f64((frag_y - half_h) as f64).mul(step_width);
    let mid = NumVec3 {
        x: R::from_split(scene.mid_x),
        y: R::from_split(scene.mid_y),
        z: R::from_split(scene.mid_z),
    };

    let origin = mid
        .add(cam_forward.scale(R::from_split(scene.z_start_delta)))
        .add(cam_right.scale(x_offset))
        .add(cam_up.scale(y_offset));

    (origin, dir)
}

fn build_march_params_from_ds_uploads<R: PortNum>(scene: &GpuDsUploads) -> MarchParams<R> {
    MarchParams {
        step_width: R::from_split(scene.step_width),
        max_ray_length: R::from_split(scene.max_ray_length),
        de_stop: R::from_split(scene.de_stop),
        de_stop_factor: R::from_split(scene.de_stop_factor),
        s_z_step_div: R::from_split(scene.s_z_step_div),
        ms_de_sub: R::from_split(scene.ms_de_sub),
        mct_mh04_zsd: R::from_split(scene.mct_mh04_zsd),
        de_floor: R::from_split(scene.de_floor),
        max_iters: scene.max_iters,
        bin_search_steps: scene.bin_search_steps,
        first_step_random: scene.first_step_random,
        d_fog_on_it: scene.d_fog_on_it,
    }
}

fn scene_destop_at_steps_num<R: PortNum>(params: &MarchParams<R>, depth_steps: R) -> R {
    params
        .de_stop
        .mul(R::one().add(depth_steps.abs().mul(params.de_stop_factor)))
}

fn num_max<R: PortNum>(a: R, b: R) -> R {
    if a.cmp(b) == std::cmp::Ordering::Less {
        b
    } else {
        a
    }
}

fn split_vec3_to_num<R: PortNum>(v: F3Split) -> NumVec3<R> {
    NumVec3 {
        x: R::from_split(v.x),
        y: R::from_split(v.y),
        z: R::from_split(v.z),
    }
}

fn numvec3_to_vec3<R: PortNum>(v: NumVec3<R>) -> render::Vec3 {
    render::Vec3::new(v.x.to_f64(), v.y.to_f64(), v.z.to_f64())
}

fn calc_de_scene_num<R: PortNum>(
    scene: &OrbitScene<R>,
    params: &MarchParams<R>,
    pos: NumVec3<R>,
) -> (i32, R) {
    let (iters, de) = hybrid_de_scene(scene, pos.x, pos.y, pos.z);
    (iters as i32, num_max(de, params.de_floor))
}

fn ray_march_scene_num<R: PortNum>(
    scene: &OrbitScene<R>,
    params: &MarchParams<R>,
    origin: NumVec3<R>,
    dir: NumVec3<R>,
    seed0: u32,
) -> MarchResult<R> {
    let mut t = R::zero();
    let mut last_de: R;
    let mut last_step: R;
    let mut rsfmul = R::one();
    let mut step_count = 0.0f64;
    let mut seed = seed0;
    let mut first_step = params.first_step_random;

    let pos = origin.add(dir.scale(t));
    let (iters, de) = calc_de_scene_num(scene, params, pos);
    let current_destop = scene_destop_at_steps_num(params, t.div(params.step_width));
    if iters >= params.max_iters || de.cmp(current_destop) == std::cmp::Ordering::Less {
        return MarchResult::Hit {
            depth: t,
            iters,
            shadow_steps: step_count.round().clamp(0.0, 1023.0) as i32,
        };
    }

    last_step = num_max(
        de.mul(params.s_z_step_div),
        params.step_width.mul_f64(0.11),
    );
    last_de = de;

    for _ in 0..2_000_000 {
        let current_destop = scene_destop_at_steps_num(params, t.div(params.step_width));
        let pos = origin.add(dir.scale(t));
        let (iters, mut de) = calc_de_scene_num(scene, params, pos);

        let max_de = last_de.add(last_step);
        if de.cmp(max_de) == std::cmp::Ordering::Greater {
            de = max_de;
        }

        if iters < params.max_iters && de.cmp(current_destop) != std::cmp::Ordering::Less {
            let mut step = num_max(
                de.sub(params.ms_de_sub.mul(current_destop))
                    .mul(params.s_z_step_div)
                    .mul(rsfmul),
                params.step_width.mul_f64(0.11),
            );
            let max_step_here = num_max(current_destop, params.step_width.mul_f64(0.4))
                .mul(params.mct_mh04_zsd);

            if max_step_here.cmp(step) == std::cmp::Ordering::Less {
                if params.d_fog_on_it == 0 || iters == params.d_fog_on_it {
                    step_count += max_step_here.to_f64() / step.to_f64().max(1.0e-300);
                }
                step = max_step_here;
            } else if params.d_fog_on_it == 0 || iters == params.d_fog_on_it {
                step_count += 1.0;
            }

            if first_step {
                seed = seed.wrapping_mul(214013).wrapping_add(2531011);
                first_step = false;
                let jitter = ((seed & 0x7fff_ffff) as f64) * (1.0 / 2147483647.0);
                step = step.mul_f64(jitter);
            }

            if last_de.cmp(de.add_f64(1.0e-30)) == std::cmp::Ordering::Greater {
                let ratio = last_step.to_f64() / last_de.sub(de).to_f64();
                rsfmul = if ratio < 1.0 {
                    R::from_f64(ratio.max(0.5))
                } else {
                    R::one()
                };
            } else {
                rsfmul = R::one();
            }

            last_de = de;
            last_step = step;
            t = t.add(step);

            if t.cmp(params.max_ray_length) == std::cmp::Ordering::Greater {
                return MarchResult::Miss;
            }
        } else {
            let mut refine_step = last_step.mul_f64(-0.5);
            for _ in 0..params.bin_search_steps {
                t = t.add(refine_step);
                let rpos = origin.add(dir.scale(t));
                let destop_here = scene_destop_at_steps_num(params, t.div(params.step_width));
                let (ri, rd) = calc_de_scene_num(scene, params, rpos);
                if rd.cmp(destop_here) == std::cmp::Ordering::Less || ri >= params.max_iters {
                    refine_step = refine_step.abs().mul_f64(-0.55);
                } else {
                    refine_step = refine_step.abs().mul_f64(0.55);
                }
            }

            let hit_pos = origin.add(dir.scale(t));
            let (final_iters, _) = calc_de_scene_num(scene, params, hit_pos);
            return MarchResult::Hit {
                depth: t,
                iters: final_iters,
                shadow_steps: step_count.round().clamp(0.0, 1023.0) as i32,
            };
        }
    }

    MarchResult::Miss
}
fn render_selfcontained_shaderlike_pixels<R: PortNum + Send + Sync>(
    scene: &CathedralScene,
    width: usize,
) -> Result<(Vec<u8>, usize, usize, usize), String> {
    let scale = (width as f64 / scene.base_width).max(0.001);
    let mut params = render::RenderParams::from_m3p(&scene.m3p);
    params.apply_image_scale(scale);
    let width = params.camera.width as usize;
    let height = params.camera.height as usize;

    let uploads = build_ds_uploads(scene, width);
    let orbit = orbit_scene_from_ds_uploads::<R>(&uploads);
    let march = build_march_params_from_ds_uploads::<R>(&uploads);
    let lighting_state = build_standalone_lighting_state(&scene.m3p.lighting, &params.camera, &params);
    let soft_hs_light = standalone_soft_hs_light_dir(&scene.m3p.lighting, &params.camera, &params);

    let mut pixels = vec![0u8; width * height * 4];
    let mut hits = 0usize;

    for y in 0..height {
        let y_pos = (y as f64 + 0.5) / height.max(1) as f64;
        for x in 0..width {
            let (origin, dir) = shaderlike_ray_for_pixel_num::<R>(&uploads, width, height, x, y);
            let seed = (x as u32)
                .wrapping_mul(0x45d9f3b)
                .wrapping_add((y as u32).wrapping_mul(0x2710_0001))
                .wrapping_add(0x2456_3487);
            let hit = ray_march_scene_num(&orbit, &march, origin, dir, seed);
            match hit {
                MarchResult::Hit {
                    depth,
                    iters,
                    shadow_steps,
                } => {
                    hits += 1;
                    let depth_f64 = depth.to_f64();
                    let hit_pos_num = origin.add(dir.scale(depth));
                    let _hit_pos = numvec3_to_vec3(hit_pos_num);
                    let ray_dir = numvec3_to_vec3(dir).normalize();
                    let surface = standalone_surface_sample_num(
                        &uploads,
                        &orbit,
                        &march,
                        hit_pos_num,
                        depth_f64,
                    );

                    let mut shadow_word = shadow_steps & 0x3ff;
                    if let Some((_li, light_dir, i_light_pos)) = soft_hs_light {
                        shadow_word |= 0xFC00;
                        let soft_bits = standalone_soft_hs_bits_num(
                            &uploads,
                            &orbit,
                            &march,
                            hit_pos_num,
                            depth_f64,
                            ray_dir,
                            surface.normal,
                            light_dir,
                            i_light_pos,
                            y,
                            width,
                            height,
                        );
                        shadow_word = (shadow_word & 0x03FF) | (soft_bits << 10);
                    }

                    let final_ao = standalone_deao_num(
                        &uploads,
                        &orbit,
                        hit_pos_num,
                        surface.normal,
                        depth_f64,
                        x as i32,
                        y as i32,
                        width,
                        height,
                        &scene.m3p.ssao,
                    );

                    let color = standalone_shade_with_final_ao_mb3d(
                        &lighting_state,
                        &scene.m3p.ssao,
                        &params,
                        surface.normal,
                        surface.roughness,
                        ray_dir.scale(-1.0),
                        iters,
                        shadow_word,
                        final_ao,
                        depth_f64,
                        y_pos,
                        params.max_ray_length,
                    );
                    let idx = (y * width + x) * 4;
                    pixels[idx] = color[0];
                    pixels[idx + 1] = color[1];
                    pixels[idx + 2] = color[2];
                    pixels[idx + 3] = 255;
                }
                MarchResult::Miss => {
                    let idx = (y * width + x) * 4;
                    pixels[idx] = 10;
                    pixels[idx + 1] = 10;
                    pixels[idx + 2] = 15;
                    pixels[idx + 3] = 255;
                }
            }
        }
    }

    Ok((pixels, width, height, hits))
}

fn offset_num_pos<R: PortNum>(base: NumVec3<R>, dir: render::Vec3, scale: f64) -> NumVec3<R> {
    NumVec3 {
        x: base.x.add_f64(dir.x * scale),
        y: base.y.add_f64(dir.y * scale),
        z: base.z.add_f64(dir.z * scale),
    }
}

fn calc_de_scene_num_at_numpos<R: PortNum>(
    orbit: &OrbitScene<R>,
    march: &MarchParams<R>,
    pos: NumVec3<R>,
) -> f64 {
    calc_de_scene_num(orbit, march, pos).1.to_f64()
}

fn standalone_surface_sample_num<R: PortNum>(
    uploads: &GpuDsUploads,
    orbit: &OrbitScene<R>,
    march: &MarchParams<R>,
    hit_pos: NumVec3<R>,
    depth: f64,
) -> render::SurfaceSampleMb3d {
    let step_width = split_to_f64(uploads.step_width).max(1.0e-300);
    let de_stop_header = split_to_f64(uploads.de_stop_header);
    let de_stop_factor = split_to_f64(uploads.de_stop_factor);
    let forward = split_vec3_to_vec3(uploads.cam_forward).normalize();
    let right = split_vec3_to_vec3(uploads.cam_right).normalize();
    let up = split_vec3_to_vec3(uploads.cam_up).normalize();

    let m_zz = depth / step_width;
    let n_offset = de_stop_header.min(1.0) * (1.0 + m_zz.abs() * de_stop_factor) * 0.15 * step_width;

    let fwd = forward.scale(n_offset);
    let rt = right.scale(n_offset);
    let upv = up.scale(n_offset);

    let dz = calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, forward, n_offset))
        - calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, forward, -n_offset));
    let dx = calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, right, n_offset))
        - calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, right, -n_offset));
    let dy = calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, up, n_offset))
        - calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, up, -n_offset));

    let normal_basis = render::Vec3::new(dx, dy, dz);
    let normal_coarse = rt
        .normalize()
        .scale(dx)
        .add(upv.normalize().scale(dy))
        .add(fwd.normalize().scale(dz))
        .normalize();

    let smooth_n = uploads.sm_normals.min(8);
    if smooth_n <= 0 {
        return render::SurfaceSampleMb3d {
            normal: normal_coarse,
            roughness: 0.0,
        };
    }

    let noffset2 = n_offset * 2.0;
    let step_snorm = noffset2 * 3.0 / (smooth_n as f64 + 0.5);
    if step_snorm <= 1.0e-30 {
        return render::SurfaceSampleMb3d {
            normal: normal_coarse,
            roughness: 0.0,
        };
    }

    let create_xy_vecs_from_normals_mb3d = |n: render::Vec3| {
        let d = n.y * n.y + n.x * n.x;
        if d < 1.0e-50 {
            return (render::Vec3::new(1.0, 0.0, 0.0), render::Vec3::new(0.0, 1.0, 0.0));
        }
        let denom = (d + n.z * n.z + 1.0e-100).sqrt();
        let half_angle = (-n.z / denom).clamp(-1.0, 1.0).acos() * 0.5;
        let (mut sin_a, cos_a) = half_angle.sin_cos();
        sin_a /= d.sqrt();
        let d0 = -n.y * sin_a;
        let d1 = n.x * sin_a;
        let vx = render::Vec3::new(1.0 - 2.0 * d1 * d1, 2.0 * d0 * d1, 2.0 * d1 * cos_a);
        let vy = render::Vec3::new(vx.y, 1.0 - 2.0 * d0 * d0, -2.0 * d0 * cos_a);
        (vx, vy)
    };
    let rotate_vector_reverse_basis = |v: render::Vec3| {
        right
            .normalize()
            .scale(v.x)
            .add(up.normalize().scale(v.y))
            .add(forward.normalize().scale(v.z))
    };

    let mut dnn = calc_de_scene_num_at_numpos(orbit, march, hit_pos);
    if smooth_n < 8 {
        dnn = (
            dnn
                + calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, right, -noffset2))
                + calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, right, noffset2))
                + calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, up, -noffset2))
                + calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, up, noffset2))
        ) * 0.2;
    }

    let (vx_basis, vy_basis) = create_xy_vecs_from_normals_mb3d(normal_basis);
    let vx = rotate_vector_reverse_basis(vx_basis).normalize();
    let vy = rotate_vector_reverse_basis(vy_basis).normalize();
    let mut nn1 = 0.0;
    let mut nn2 = 0.0;
    let mut ds1 = 0.0;
    let mut ds2 = 0.0;

    for it in -smooth_n..=smooth_n {
        if it == 0 {
            continue;
        }
        let t = it as f64 * step_snorm;
        let de_x = calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, vx, t));
        let dt = (de_x - dnn) / it as f64;
        nn1 += dt;
        ds1 += dt * dt;
    }
    for it in -smooth_n..=smooth_n {
        if it == 0 {
            continue;
        }
        let t = it as f64 * step_snorm;
        let de_y = calc_de_scene_num_at_numpos(orbit, march, offset_num_pos(hit_pos, vy, t));
        let dt = (de_y - dnn) / it as f64;
        nn2 += dt;
        ds2 += dt * dt;
    }

    let d_m = (smooth_n * 2) as f64;
    let d_t2 = noffset2 * 0.5 / (d_m * step_snorm).max(1.0e-30);
    let mut d_sg = ds1 * d_m - nn1 * nn1;
    d_sg += ds2 * d_m - nn2 * nn2;

    let denom = 1.0e-40 + normal_basis.dot(normal_basis);
    let mut rough = ((d_sg * 7.0 * d_t2 * d_t2) / denom).max(0.0).sqrt() - 0.05;
    rough = rough.clamp(0.0, 1.0);

    let out_n = rotate_vector_reverse_basis(render::Vec3::new(
        normal_basis.x + nn1 * d_t2,
        normal_basis.y + nn2 * d_t2,
        normal_basis.z,
    ))
    .normalize();

    render::SurfaceSampleMb3d {
        normal: out_n,
        roughness: rough,
    }
}

fn standalone_soft_hs_bits_num<R: PortNum>(
    uploads: &GpuDsUploads,
    orbit: &OrbitScene<R>,
    march: &MarchParams<R>,
    hit_pos: NumVec3<R>,
    depth_world: f64,
    ray_dir: render::Vec3,
    normal: render::Vec3,
    light_dir: render::Vec3,
    i_light_pos: u8,
    y: usize,
    width: usize,
    height: usize,
) -> i32 {
    let step_width = split_to_f64(uploads.step_width).max(1.0e-30);
    let max_ray_length = split_to_f64(uploads.max_ray_length);
    let fov_y = uploads.fov_y as f64;
    let hs_max_length_multiplier = split_to_f64(uploads.hs_max_length_multiplier).max(1.0e-30);
    let soft_shadow_radius = split_to_f64(uploads.soft_shadow_radius).max(1.0e-30);
    let s_z_step_div_raw = split_to_f64(uploads.s_z_step_div_raw);
    let de_stop = split_to_f64(uploads.de_stop);
    let de_stop_factor = split_to_f64(uploads.de_stop_factor);
    let ms_de_sub = split_to_f64(uploads.ms_de_sub);
    let mct_mh04_zsd = split_to_f64(uploads.mct_mh04_zsd);

    let view_dir = ray_dir.normalize();

    let mut refined_pos = hit_pos;
    let mut refined_depth = depth_world.max(0.0);
    let mut refine_step = step_width;
    for _ in 0..8 {
        let de_ref = calc_de_scene_num_at_numpos(orbit, march, refined_pos);
        let de_stop_ref = de_stop * (1.0 + (refined_depth / step_width).abs() * de_stop_factor);
        if de_ref <= de_stop_ref {
            refined_pos = offset_num_pos(refined_pos, view_dir, -refine_step);
            refined_depth = (refined_depth - refine_step).max(0.0);
        } else {
            refined_pos = offset_num_pos(refined_pos, view_dir, refine_step);
            refined_depth += refine_step;
        }
        refine_step *= 0.5;
    }

    let mut depth_steps = refined_depth / step_width - 0.1;
    if depth_steps < 0.0 {
        depth_steps = 0.0;
    }
    let mut pos = offset_num_pos(refined_pos, view_dir, -0.1 * step_width);

    let zz = depth_steps.abs();
    let zend_steps = (max_ray_length / step_width).max(1.0e-30);
    let fov_y_rad = fov_y * std::f64::consts::PI / 180.0;
    let max_l_hs = (width as f64 + y as f64)
        * 0.6
        * (1.0 + 0.5 * zz.min(zend_steps * 0.4) * fov_y_rad.max(0.0) / (height as f64).max(1.0))
        * hs_max_length_multiplier;
    if max_l_hs <= 0.0 {
        return 63;
    }

    let is_positional = (i_light_pos & 1) != 0;
    if is_positional {
        return 63;
    }

    let mut zr_soft = 1.0f64;
    let zr_s_mul = 80.0 / soft_shadow_radius;
    let n = normal.normalize();
    let l = light_dir.normalize();
    let v = view_dir;
    let hs_vec = l.scale(-1.0);
    let zz2mul = -hs_vec.dot(v);

    if n.dot(hs_vec) > 0.0 {
        return 0;
    }

    let mut d_t1 = max_l_hs;
    let mut zz2_steps = depth_steps;
    let mut ms_de_stop_world = de_stop * (1.0 + zz2_steps.abs() * de_stop_factor);
    let mut step_factor_diff = 1.0f64;
    let mut de_world = calc_de_scene_num_at_numpos(orbit, march, pos);

    loop {
        let r_last_de_world = de_world;
        let max_step_world = (ms_de_stop_world.max(0.4 * step_width)) * mct_mh04_zsd;
        let r_last_step_world = ((de_world - ms_de_sub * ms_de_stop_world)
            * s_z_step_div_raw
            * step_factor_diff)
            .max(0.11 * step_width)
            .min(max_step_world);
        if r_last_step_world <= 0.0 {
            break;
        }

        let r_last_step_width = r_last_step_world / step_width;
        d_t1 -= r_last_step_width;
        pos = offset_num_pos(pos, l, r_last_step_world);
        zz2_steps += r_last_step_width * zz2mul;
        ms_de_stop_world = de_stop * (1.0 + zz2_steps.abs() * de_stop_factor);

        de_world = calc_de_scene_num_at_numpos(orbit, march, pos);
        let traveled = (max_l_hs - d_t1).max(0.0);
        let soft_term =
            ((de_world - ms_de_stop_world) / step_width) * zr_s_mul / (traveled + 0.11)
                + (traveled / max_l_hs.max(1.0e-30)).powi(8);
        zr_soft = zr_soft.min(soft_term);

        if de_world <= ms_de_stop_world {
            break;
        }
        if de_world > r_last_de_world + r_last_step_world {
            de_world = r_last_de_world + r_last_step_world;
        }
        if r_last_de_world > de_world + 1.0e-30 {
            let s_tmp = r_last_step_world / (r_last_de_world - de_world);
            if s_tmp < 1.0 {
                step_factor_diff = s_tmp.max(0.5);
            } else {
                step_factor_diff = 1.0;
            }
        } else {
            step_factor_diff = 1.0;
        }
        if d_t1 < 0.0 {
            break;
        }
    }

    (zr_soft.clamp(0.0, 1.0) * 63.4)
        .round()
        .clamp(0.0, 63.0) as i32
}

fn standalone_ao_step_jitter(pixel_x: i32, pixel_y: i32, ray_idx: usize) -> f64 {
    let mut v = (pixel_x as u32).wrapping_mul(73_856_093)
        ^ (pixel_y as u32).wrapping_mul(19_349_663)
        ^ (ray_idx as u32).wrapping_mul(83_492_791);
    v ^= v >> 13;
    v = v.wrapping_mul(1_274_126_177);
    (v as f64) / (u32::MAX as f64)
}

fn calc_raw_de_scene_num_at_numpos<R: PortNum>(orbit: &OrbitScene<R>, pos: NumVec3<R>) -> f64 {
    hybrid_de_scene(orbit, pos.x, pos.y, pos.z).1.to_f64()
}

fn standalone_deao_num<R: PortNum>(
    uploads: &GpuDsUploads,
    orbit: &OrbitScene<R>,
    hit_pos: NumVec3<R>,
    normal: render::Vec3,
    depth: f64,
    pixel_x: i32,
    pixel_y: i32,
    width: usize,
    height: usize,
    ssao: &m3p::M3PSSAO,
) -> f64 {
    let mut final_ao = 1.0f64;
    if !(ssao.calc_amb_shadow && ssao.mode == 3) {
        return final_ao;
    }

    let step_width = split_to_f64(uploads.step_width).max(1.0e-300);
    let de_stop_factor = split_to_f64(uploads.de_stop_factor);
    let de_stop_header = split_to_f64(uploads.de_stop_header).max(1.0e-300);
    let de_scale = split_to_f64(uploads.de_scale);
    let m_zz = depth / step_width;

    let normal_basis_w = normal.normalize();
    let normal_basis_u = if normal_basis_w.x.abs() > 0.1 {
        render::Vec3::new(0.0, 1.0, 0.0).cross(normal_basis_w).normalize()
    } else {
        render::Vec3::new(1.0, 0.0, 0.0).cross(normal_basis_w).normalize()
    };
    let normal_basis_v = normal_basis_w.cross(normal_basis_u);
    let make_world_dir = |polar: f64, azimuth: f64| {
        let (sy, cy) = polar.sin_cos();
        let (sz, cz) = azimuth.sin_cos();
        let local_dir = render::Vec3::new(sy * cz, sy * sz, cy);
        normal_basis_u
            .scale(local_dir.x)
            .add(normal_basis_v.scale(local_dir.y))
            .add(normal_basis_w.scale(local_dir.z))
            .normalize()
    };

    let (dither_y, dither_x) = if ssao.ao_dithering > 0 {
        let denom = ssao.ao_dithering as f64;
        (
            (pixel_y.rem_euclid(ssao.ao_dithering + 1) as f64) * 0.5 / denom,
            (pixel_x.rem_euclid(ssao.ao_dithering + 1) as f64) * 0.5 / denom,
        )
    } else {
        (0.25, 0.0)
    };

    let mut rot_m = Vec::new();
    let (_row_abr, d_step_mul, d_min_a_dif, correction_weight) = if ssao.quality == 0 {
        let row_abr = std::f64::consts::PI / 3.0;
        let polar = if ssao.ao_dithering > 0 {
            (dither_y + 0.5) * 50.0_f64.to_radians()
        } else {
            row_abr * 0.5
        };
        for itmp in 0..3 {
            let azimuth = (itmp as f64 + dither_x) * std::f64::consts::PI * 2.0 / 3.0;
            rot_m.push(make_world_dir(polar, azimuth));
        }
        (row_abr, 1.8, -1.0, 0.3)
    } else {
        let row_abr = std::f64::consts::PI * 0.5 / (ssao.quality as f64 + 0.9);
        if dither_y >= 0.1 {
            rot_m.push(normal_basis_w);
        }
        for iy in 1..=ssao.quality {
            let row_count = ((iy as f64 * row_abr).sin() * std::f64::consts::PI * 2.0 / row_abr)
                .round()
                .max(1.0) as i32;
            let polar = row_abr * (iy as f64 + dither_y - 0.25);
            for ix in 0..row_count {
                let azimuth =
                    (ix as f64 + dither_x) * std::f64::consts::PI * 2.0 / row_count as f64;
                rot_m.push(make_world_dir(polar, azimuth));
            }
        }
        let correction_weight = if ssao.quality == 1 { 0.2 } else { 0.1666 };
        (
            row_abr,
            1.0 + row_abr.sin(),
            (row_abr * 1.2).cos(),
            correction_weight,
        )
    };

    if uploads.adaptive_ao_subsampling && rot_m.len() >= 12 {
        let mut write = 0usize;
        for read in (0..rot_m.len()).step_by(2) {
            rot_m[write] = rot_m[read];
            write += 1;
        }
        rot_m.truncate(write);
    }

    let ray_count = rot_m.len();
    if ray_count == 0 {
        return final_ao;
    }

    let mut min_ra = vec![0.0f64; ray_count];
    let mut s_add = vec![0.0f64; ray_count];

    let de_mul = ((ray_count as f64) * 0.5).sqrt();
    let overlap_abr = 1.2 / (1.0 / de_mul).asin();
    let step_ao = 1.0 + m_zz.abs() * de_stop_factor;
    let s_max_d = ssao.deao_max_l as f64
        * 0.5
        * ((width * width + height * height) as f64).sqrt();

    let mut ms_de_stop_steps = de_stop_header * step_ao;
    if ms_de_stop_steps > 10000.0 {
        ms_de_stop_steps = 10000.0;
    }
    if ms_de_stop_steps < de_stop_header {
        ms_de_stop_steps = de_stop_header;
    }

    let step_ao_actual = ms_de_stop_steps / de_stop_header;
    let max_dist_steps = s_max_d * step_ao_actual.sqrt();
    let ms_de_stop = if uploads.b_vary_de_stop {
        ms_de_stop_steps / (d_step_mul * d_step_mul)
    } else {
        de_stop_header / (d_step_mul * d_step_mul)
    };

    for i in 0..ray_count {
        let s_vec = rot_m[i];
        let mut dt1 = step_ao_actual * d_step_mul;
        let mut s_tmp = 1.0f64;
        let mut b_first_step = uploads.first_step_random;

        loop {
            let mut b_end = false;

            if b_first_step {
                b_first_step = false;
                dt1 *= standalone_ao_step_jitter(pixel_x, pixel_y, i) * 1.5 + 0.5;
            } else if dt1 > max_dist_steps {
                dt1 = max_dist_steps;
                b_end = true;
            }

            let probe_pos = offset_num_pos(hit_pos, s_vec, dt1 * step_width);
            let de_world = calc_raw_de_scene_num_at_numpos(orbit, probe_pos);
            let dt2 = de_world * de_scale / step_width;

            let md_d10 = 0.1 / (max_dist_steps * de_mul);
            let val = ((dt2 - ms_de_stop) / dt1 + md_d10).min(s_tmp);
            if val < s_tmp {
                s_tmp = val;
            }

            if s_tmp < 0.02 {
                break;
            }

            let step_add = if dt2 > dt1 * d_step_mul {
                dt2
            } else {
                dt1 * d_step_mul
            };
            dt1 += step_add;

            if b_end {
                break;
            }
        }

        min_ra[i] = s_tmp.max(0.0) * de_mul;
    }

    let mut final_ao_val = 0.0f64;
    for iy in 0..ray_count {
        let max_add = 1.0 - min_ra[iy];
        if max_add > 0.0 {
            for ix in 0..ray_count {
                if ix != iy {
                    let d_tmp = rot_m[iy].dot(rot_m[ix]);
                    if d_tmp > d_min_a_dif {
                        let overlap = min_ra[ix] - d_tmp.acos() * overlap_abr + 1.0;
                        if overlap > 0.0 {
                            s_add[iy] += max_add.min(overlap) * correction_weight;
                        }
                    }
                }
            }
        }
    }

    for iy in 0..ray_count {
        final_ao_val += (s_add[iy] + min_ra[iy]).min(1.0);
    }

    let amb_shadow_norm = (1.0 - final_ao_val / ray_count as f64).clamp(0.0, 1.0);
    let s_amplitude = ssao.amb_shad;
    let mut d_amb_s = if s_amplitude > 1.0 {
        let mut d = 1.0 - amb_shadow_norm;
        d = d + (s_amplitude - 1.0) * (d * d - d);
        d
    } else {
        1.0 - s_amplitude * amb_shadow_norm
    };
    d_amb_s = d_amb_s.clamp(0.0, 1.0);
    final_ao = d_amb_s;

    final_ao
}

#[test]
#[ignore = "manual render of self-contained generic ds shaderlike path at 1920x1080"]
fn render_selfcontained_ds_1080p() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 1920usize;
    let (pixels, out_w, out_h, hits) =
        render_selfcontained_shaderlike_pixels::<Ds>(&scene, width)
            .expect("self-contained ds pixels should render");
    let output_path = "/tmp/mb3d_selfcontained_ds_1920x1080.png";
    encode_png(output_path, &pixels, out_w, out_h).expect("self-contained ds png should encode");
    println!(
        "self-contained ds render: {}x{} hits={} output={}",
        out_w, out_h, hits, output_path
    );
}

#[test]
#[ignore = "manual render of self-contained generic ds shaderlike path at 480x270"]
fn render_selfcontained_ds_25pct() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 480usize;
    let (pixels, out_w, out_h, hits) =
        render_selfcontained_shaderlike_pixels::<Ds>(&scene, width)
            .expect("self-contained ds pixels should render");
    let output_path = "/tmp/mb3d_selfcontained_ds_480x270.png";
    encode_png(output_path, &pixels, out_w, out_h).expect("self-contained ds png should encode");
    println!(
        "self-contained ds render: {}x{} hits={} output={}",
        out_w, out_h, hits, output_path
    );
}

#[test]
#[ignore = "manual render of self-contained generic ds shaderlike path at 960xH"]
fn render_selfcontained_ds_960p() {
    let path = format!("{}/../../local/mb3d/cathedral.m3p", env!("CARGO_MANIFEST_DIR"));
    let scene = load_cathedral_scene(&path).expect("cathedral scene should load");
    let width = 960usize;
    let (pixels, out_w, out_h, hits) =
        render_selfcontained_shaderlike_pixels::<Ds>(&scene, width)
            .expect("self-contained ds pixels should render");
    let output_path = "/tmp/mb3d_selfcontained_ds_960.png";
    encode_png(output_path, &pixels, out_w, out_h).expect("self-contained ds png should encode");
    println!(
        "self-contained ds render: {}x{} hits={} output={}",
        out_w, out_h, hits, output_path
    );
}
