use crate::formulas::{self, HybridProgram, IterParams};

const MB3D_MIN_STEP_UNITS: f64 = 0.11;

#[derive(Debug, Clone, Copy)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self { Vec3 { x, y, z } }
    pub fn dot(self, o: Self) -> f64 { self.x*o.x + self.y*o.y + self.z*o.z }
    pub fn len(self) -> f64 { self.dot(self).sqrt() }
    pub fn normalize(self) -> Self {
        let l = self.len();
        if l > 1e-30 { Vec3::new(self.x/l, self.y/l, self.z/l) } else { self }
    }
    pub fn scale(self, s: f64) -> Self { Vec3::new(self.x*s, self.y*s, self.z*s) }
    pub fn add(self, o: Self) -> Self { Vec3::new(self.x+o.x, self.y+o.y, self.z+o.z) }
    pub fn sub(self, o: Self) -> Self { Vec3::new(self.x-o.x, self.y-o.y, self.z-o.z) }
    pub fn cross(self, o: Self) -> Self {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
}

/// Camera built from M3P header parameters, matching MB3D's CalcVGradsFromHeader8rots
/// and GetMCTparasFromHeader.
///
/// MB3D computes view vectors from rotation angles, then normalizes them to StepWidth
/// magnitude. The pixel at (px, py) starts at:
///   Ystart + py * Vgrads[1] + px * Vgrads[0]
/// where Ystart = camera + z1*forward - halfH*up - halfW*right
/// and z1 = (z_start - z_mid) / StepWidth
pub struct Camera {
    pub mid: Vec3,        // Xmid, Ymid, Zmid
    pub right: Vec3,      // Vgrads[0]: magnitude = StepWidth
    pub up: Vec3,         // Vgrads[1]: magnitude = StepWidth
    pub forward: Vec3,    // Vgrads[2]: magnitude = StepWidth (march direction per step)
    pub step_width: f64,  // StepWidth from header (world units per step)
    pub z_start: f64,
    pub z_end: f64,
    pub width: i32,
    pub height: i32,
    pub fov_y: f64,
}

impl Camera {
    pub fn from_m3p(m3p: &crate::m3p::M3PFile) -> Self {
        let step_width = m3p.step_width;

        // Use the view matrix directly from the file, as Euler angles might be zero
        // if the user navigated using the 3D navigator without updating them.
        let mut right   = Vec3::new(m3p.view_matrix[0][0], m3p.view_matrix[0][1], m3p.view_matrix[0][2]);
        let mut up      = Vec3::new(m3p.view_matrix[1][0], m3p.view_matrix[1][1], m3p.view_matrix[1][2]);
        let mut forward = Vec3::new(m3p.view_matrix[2][0], m3p.view_matrix[2][1], m3p.view_matrix[2][2]);

        // MB3D's NormVGrads: normalizes the matrix to StepWidth
        right = right.normalize().scale(step_width);
        up = up.normalize().scale(step_width);
        forward = forward.normalize().scale(step_width);

        let mid = Vec3::new(m3p.x_mid, m3p.y_mid, m3p.z_mid);
        let z_start = m3p.z_start; // Already in world units in the file

        Camera {
            mid,
            right,
            up,
            forward,
            step_width,
            z_start,
            z_end: m3p.z_end,
            width: m3p.width,
            height: m3p.height,
            fov_y: m3p.fov_y,
        }
    }

}

pub struct RenderParams {
    pub camera: Camera,
    pub iter_params: IterParams,
    pub adaptive_ao_subsampling: bool,
    pub max_ray_length: f64,   // maximum ray length in world units
    pub de_stop: f64,          // DE threshold for surface hit (world units, adjusted by de_scale)
    pub s_z_step_div: f64,     // effective step multiplier: sZstepDiv * de_scale
    pub s_z_step_div_raw: f64,
    pub b_dfog_it: u8,
    pub d_fog_on_it: u16,
    pub first_step_random: bool,
    pub b_vol_light_nr: u8,
    pub b_calculate_hard_shadow: u8,
    pub b_hs_calculated: u8,
    pub b_calc1_hs_soft: u8,
    pub soft_shadow_radius: f64,
    pub hs_max_length_multiplier: f64,
    pub ms_de_sub: f64,        // MCTparas.msDEsub
    pub step_width: f64,       // StepWidth from header
    pub de_stop_factor: f64,   // mctDEstopFactor: how DEstop scales with distance
    pub mct_mh04_zsd: f64,     // Max(iMandWidth, iMandHeight) ray-step limiter
    pub de_floor: f64,         // minimum DE value (0.25 * de_stop)
    pub de_scale: f64,         // dDEscale_computed: formula-specific DE scaling factor
    pub bin_search_steps: i32, // iDEAddSteps for binary search refinement
    pub z_corr: f64,
    pub b_vary_de_stop: bool,
    pub z_cmul: f64,
    pub de_stop_header: f64,
    pub sm_normals: i32,
}

/// Compute dDEscale factor for hybrid formulas.
/// This matches MB3D's GetMCTparasFromHeader (HeaderTrafos.pas lines 686-772).
///
/// The dDEscale is a weighted average across all formulas in the hybrid:
/// - For DEoption in [5,11] (AmazBox-type): contribution = (scale*1.2+1)/scale * iter_count
/// - For other analytic DE formulas: contribution = dADEscale * iter_count
/// Final: dDEscale = total_contribution / total_iter_count
fn compute_de_scale(m3p: &crate::m3p::M3PFile) -> f64 {
    let addon = &m3p.addon;
    let end_to = (addon.b_hyb_opt1 & 7) as usize;
    let mut total_scale = 0.0;
    let mut total_weight = 0.0;

    for i in 0..=end_to.min(5) {
        let f = &addon.formulas[i];
        let iter_count = f.iteration_count.abs() as f64;
        if iter_count == 0.0 {
            continue;
        }

        let formula_scale = match f.formula_nr {
            // HeaderTrafos: if j in [5,11], x1 += (scale * 1.2 + 1) / scale
            4 => {
                let scale = f.option_values[0];
                if scale < 0.0 {
                    0.65
                } else if scale.abs() > 1.0e-30 {
                    (scale * 1.2 + 1.0) / scale
                } else {
                    1.0
                }
            }
            // We do not yet have the full built-in dADEscale table.
            // Falling back to 1.0 keeps the marcher stable while still
            // honoring the dominant Amazing Box contribution in this hybrid.
            _ => 1.0,
        };

        total_scale += formula_scale * iter_count;
        total_weight += iter_count;
    }

    if total_weight > 0.0 {
        total_scale / total_weight
    } else {
        1.0
    }
}

impl RenderParams {
    pub fn from_m3p(m3p: &crate::m3p::M3PFile) -> Self {
        let camera = Camera::from_m3p(m3p);
        let step_width = m3p.step_width;

        // Max ray length = (z_end - z_start) in world units
        let max_ray_length = (m3p.z_end - m3p.z_start).abs();

        // DEstop: from header, in step units. Convert to world units.
        let de_stop_header = (m3p.de_stop as f64).max(0.0001);
        let de_stop_world = de_stop_header * step_width;

        // sZstepDiv: directly from header (NOT inverted!)
        let mut s_z_step_div_raw = (m3p.z_step_div as f64).max(0.0001);
        let mut ms_de_sub = 0.0;
        if (m3p.i_options & 4) != 0 {
            s_z_step_div_raw = s_z_step_div_raw * s_z_step_div_raw
                + (1.2 * s_z_step_div_raw) * (1.0 - s_z_step_div_raw);
            ms_de_sub = s_z_step_div_raw.sqrt().min(0.9);
        }

        // Compute dDEscale factor from formula parameters
        // MB3D: dDEscale = computed_scale / StepWidth (for IsCustomDE)
        // Then: world_step = DE_world * dDEscale * sZstepDiv * StepWidth
        //                  = DE_world * computed_scale * sZstepDiv
        // So effective step multiplier = sZstepDiv * computed_scale
        // And effective DEstop = DEstop_world / computed_scale
        let de_scale = compute_de_scale(m3p);

        // Apply de_scale to step multiplier and DEstop threshold
        let s_z_step_div = s_z_step_div_raw * de_scale;
        let de_stop = de_stop_world / de_scale;

        // mctDEstopFactor: controls how DEstop grows with distance
        let fov_y_rad = m3p.fov_y * std::f64::consts::PI / 180.0;
        let x1 = 0.001 * m3p.height as f64 / (0.001f64.sin() * fov_y_rad.max(1.0 / 65535.0));
        let de_stop_factor = if m3p.b_vary_de_stop { 1.0 / x1 } else { 0.0 };

        let fov_y_rad_for_z = m3p.fov_y.max(1.0) * std::f64::consts::PI / 180.0;
        let z_corr = (fov_y_rad_for_z / m3p.height as f64).sin();
        let z_cmul = 32767.0 * 256.0 / ((((m3p.z_end - m3p.z_start) / m3p.step_width) * z_corr + 1.0).sqrt() - 0.999999999);

        // MB3D: mctMH04ZSD = max(width,height) * 0.5 * sqrt(sZstepDiv + 0.0001) * max(0.001, sRaystepLimiter)
        let s_raystep_limiter = (m3p.s_raystep_limiter as f64).max(0.001);
        let mct_mh04_zsd = m3p.width.max(m3p.height) as f64
            * 0.5
            * (s_z_step_div_raw + 0.0001).sqrt()
            * s_raystep_limiter;

        // Maximum step size (in world units) near camera; marcher uses dynamic value with current DEstop.
        // DE floor: min 0.25 * effective DEstop (in world units)
        let de_floor = de_stop * 0.25;

        // Binary search steps
        let bin_search_steps = m3p.b_steps_after_de_stop as i32;

        let d_fog_on_it = if (m3p.b_vol_light_nr & 7) > 0 {
            65535
        } else {
            m3p.b_dfog_it as u16
        };
        let first_step_random = (m3p.i_options & 1) != 0;

        RenderParams {
            camera,
            iter_params: IterParams {
                max_iters: m3p.iterations,
                min_iters: m3p.min_iterations,
                rstop: m3p.rstop,
                is_julia: m3p.is_julia,
                julia_x: m3p.julia_x,
                julia_y: m3p.julia_y,
                julia_z: m3p.julia_z,
            },
            adaptive_ao_subsampling: true,
            max_ray_length,
            de_stop,
            s_z_step_div,
            s_z_step_div_raw,
            step_width,
            de_stop_factor,
            b_dfog_it: m3p.b_dfog_it,
            d_fog_on_it,
            first_step_random,
            b_vol_light_nr: m3p.b_vol_light_nr,
            b_calculate_hard_shadow: m3p.b_calculate_hard_shadow,
            b_hs_calculated: m3p.b_hs_calculated,
            b_calc1_hs_soft: m3p.b_calc1_hs_soft,
            soft_shadow_radius: m3p.soft_shadow_radius.max(0.001),
            hs_max_length_multiplier: m3p.hs_max_length_multiplier.max(0.001),
            ms_de_sub,
            mct_mh04_zsd,
            de_floor,
            de_scale,
            bin_search_steps,
            z_corr,
            b_vary_de_stop: m3p.b_vary_de_stop,
            z_cmul,
            de_stop_header,
            sm_normals: ((m3p.i_options >> 6) & 0x0F) as i32,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PixelResult {
    Hit {
        depth: f64,
        iters: i32,
        shadow_steps: i32,
    },
    Miss,
}

struct RayGrid {
    dirs: Vec<Vec3>,
    row_origins: Vec<Vec3>,
    x_offsets: Vec<Vec3>,
}

fn build_ray_grid(camera: &Camera, num_threads: usize, rows_per_thread: usize) -> RayGrid {
    let w = camera.width as usize;
    let h = camera.height as usize;
    let half_w = camera.width as f64 * 0.5;
    let half_h = camera.height as f64 * 0.5;
    let inv_step_width = 1.0 / camera.step_width;
    let r = camera.right.scale(inv_step_width);
    let u = camera.up.scale(inv_step_width);
    let f = camera.forward.scale(inv_step_width);
    let fov_rad = camera.fov_y * std::f64::consts::PI / 180.0;
    let fov_mul = fov_rad / camera.height as f64;

    let mut sin_x = vec![0.0; w];
    let mut cos_x = vec![0.0; w];
    for x in 0..w {
        let cafx = (half_w - x as f64) * fov_mul;
        let (sx, cx) = cafx.sin_cos();
        sin_x[x] = sx;
        cos_x[x] = cx;
    }

    let mut sin_y = vec![0.0; h];
    let mut cos_y = vec![0.0; h];
    for y in 0..h {
        let cafy = (y as f64 - half_h) * fov_mul;
        let (sy, cy) = cafy.sin_cos();
        sin_y[y] = sy;
        cos_y[y] = cy;
    }

    let base_start = camera
        .mid
        .add(f.scale(camera.z_start - camera.mid.z))
        .add(r.scale(-half_w * camera.step_width));
    let mut row_origins = Vec::with_capacity(h);
    for y in 0..h {
        row_origins.push(base_start.add(u.scale((y as f64 - half_h) * camera.step_width)));
    }

    let mut x_offsets = Vec::with_capacity(w);
    for x in 0..w {
        x_offsets.push(camera.right.scale(x as f64));
    }

    let mut dirs = vec![Vec3::new(0.0, 0.0, 0.0); w * h];
    let band_len = rows_per_thread * w;
    thread::scope(|s| {
        let mut workers = Vec::new();
        for (band_idx, dir_chunk) in dirs.chunks_mut(band_len).enumerate().take(num_threads) {
            let y_start = band_idx * rows_per_thread;
            let sin_x = &sin_x;
            let cos_x = &cos_x;
            let sin_y = &sin_y;
            let cos_y = &cos_y;
            let r = r;
            let u = u;
            let f = f;

            workers.push(s.spawn(move || {
                let row_count = dir_chunk.len() / w;
                for local_y in 0..row_count {
                    let y = y_start + local_y;
                    let row_offset = local_y * w;
                    for x in 0..w {
                        let v_local = Vec3::new(-sin_x[x], sin_y[y], cos_x[x] * cos_y[y]).normalize();
                        dir_chunk[row_offset + x] =
                            r.scale(v_local.x).add(u.scale(v_local.y)).add(f.scale(v_local.z)).normalize();
                    }
                }
            }));
        }

        for worker in workers {
            worker.join().unwrap();
        }
    });

    RayGrid {
        dirs,
        row_origins,
        x_offsets,
    }
}

/// Compute distance estimation at a point, with DE floor clamping.
/// MB3D's CalcDEanalytic: if DE < DEstop * 0.25 then DE = DEstop * 0.25
fn calc_de(pos: Vec3, formulas: &HybridProgram, params: &IterParams, de_floor: f64) -> (i32, f64) {
    let (iters, de) = formulas::hybrid_de((pos.x, pos.y, pos.z), formulas, params);
    (iters, de.max(de_floor))
}

/// Estimate raw normal gradient via 6-point central differences along view vectors.
/// Returns both camera-basis components (x/right, y/up, z/forward) and world-space gradient.
fn estimate_normal_grad(
    pos: Vec3,
    eps: f64,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    formulas: &HybridProgram,
    params: &IterParams,
    de_floor: f64,
) -> (Vec3, Vec3) {
    let fwd = forward.normalize().scale(eps);
    let rt = right.normalize().scale(eps);
    let upv = up.normalize().scale(eps);

    let dz = calc_de(pos.add(fwd), formulas, params, de_floor).1
           - calc_de(pos.sub(fwd), formulas, params, de_floor).1;
    let dx = calc_de(pos.add(rt), formulas, params, de_floor).1
           - calc_de(pos.sub(rt), formulas, params, de_floor).1;
    let dy = calc_de(pos.add(upv), formulas, params, de_floor).1
           - calc_de(pos.sub(upv), formulas, params, de_floor).1;

    let basis_grad = Vec3::new(dx, dy, dz);
    let world_grad = rt.normalize()
        .scale(dx)
        .add(upv.normalize().scale(dy))
        .add(fwd.normalize().scale(dz));

    (basis_grad, world_grad)
}

/// MB3D RMCalculateNormals probe offset:
/// Noffset = min(1, DEstop) * (1 + abs(mZZ) * mctDEstopFactor) * 0.15 * StepWidth.
fn mb3d_normal_offset(params: &RenderParams, m_zz: f64) -> f64 {
    params.de_stop_header.min(1.0)
        * (1.0 + m_zz.abs() * params.de_stop_factor)
        * 0.15
        * params.step_width
}

fn destop_at_steps(params: &RenderParams, depth_steps: f64) -> f64 {
    params.de_stop * (1.0 + depth_steps.abs() * params.de_stop_factor)
}

fn rotate_vector_reverse_basis(v: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Vec3 {
    right.normalize()
        .scale(v.x)
        .add(up.normalize().scale(v.y))
        .add(forward.normalize().scale(v.z))
}

fn create_xy_vecs_from_normals_mb3d(n: Vec3) -> (Vec3, Vec3) {
    let d = n.y * n.y + n.x * n.x;
    if d < 1.0e-50 {
        return (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0));
    }

    let denom = (d + n.z * n.z + 1.0e-100).sqrt();
    let half_angle = (-n.z / denom).clamp(-1.0, 1.0).acos() * 0.5;
    let (mut sin_a, cos_a) = half_angle.sin_cos();
    sin_a /= d.sqrt();
    let d0 = -n.y * sin_a;
    let d1 = n.x * sin_a;

    let vx = Vec3::new(
        1.0 - 2.0 * d1 * d1,
        2.0 * d0 * d1,
        2.0 * d1 * cos_a,
    );
    let vy = Vec3::new(
        vx.y,
        1.0 - 2.0 * d0 * d0,
        -2.0 * d0 * cos_a,
    );
    (vx, vy)
}

fn smooth_normal_mb3d(
    pos: Vec3,
    normal_grad_basis: Vec3,
    normal_grad_world: Vec3,
    n_offset: f64,
    smooth_n: i32,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    formulas: &HybridProgram,
    params: &IterParams,
    de_floor: f64,
) -> (Vec3, f64) {
    let normal = normal_grad_world.normalize();
    if smooth_n <= 0 {
        return (normal, 0.0);
    }
    let smooth_n = smooth_n.min(8);
    let noffset2 = n_offset * 2.0;
    let step_snorm = noffset2 * 3.0 / (smooth_n as f64 + 0.5);
    if step_snorm <= 1e-30 {
        return (normal, 0.0);
    }

    let mut dnn = calc_de(pos, formulas, params, de_floor).1;
    if smooth_n < 8 {
        dnn = (
            dnn
            + calc_de(pos.add(right.normalize().scale(-noffset2)), formulas, params, de_floor).1
            + calc_de(pos.add(right.normalize().scale(noffset2)), formulas, params, de_floor).1
            + calc_de(pos.add(up.normalize().scale(-noffset2)), formulas, params, de_floor).1
            + calc_de(pos.add(up.normalize().scale(noffset2)), formulas, params, de_floor).1
        ) * 0.2;
    }

    let (vx_basis, vy_basis) = create_xy_vecs_from_normals_mb3d(normal_grad_basis);
    let vx = rotate_vector_reverse_basis(vx_basis, right, up, forward).normalize();
    let vy = rotate_vector_reverse_basis(vy_basis, right, up, forward).normalize();
    let mut nn1 = 0.0;
    let mut nn2 = 0.0;
    let mut ds1 = 0.0;
    let mut ds2 = 0.0;

    for it in -smooth_n..=smooth_n {
        if it == 0 {
            continue;
        }
        let t = it as f64 * step_snorm;
        let de_x = calc_de(pos.add(vx.scale(t)), formulas, params, de_floor).1;
        let dt = (de_x - dnn) / it as f64;
        nn1 += dt;
        ds1 += dt * dt;
    }
    for it in -smooth_n..=smooth_n {
        if it == 0 {
            continue;
        }
        let t = it as f64 * step_snorm;
        let de_y = calc_de(pos.add(vy.scale(t)), formulas, params, de_floor).1;
        let dt = (de_y - dnn) / it as f64;
        nn2 += dt;
        ds2 += dt * dt;
    }

    let d_m = (smooth_n * 2) as f64;
    if d_m <= 1e-30 {
        return (normal, 0.0);
    }
    let d_t2 = noffset2 * 0.5 / (d_m * step_snorm).max(1e-30);
    let mut d_sg = ds1 * d_m - nn1 * nn1;
    d_sg += ds2 * d_m - nn2 * nn2;

    // RMCalcRoughness: rough = clamp(sqrt(max(0, dSG * 7 * dT2^2 / |N|^2)) - 0.05, 0, 1)
    let denom = 1.0e-40 + normal_grad_basis.dot(normal_grad_basis);
    let mut rough = ((d_sg * 7.0 * d_t2 * d_t2) / denom).max(0.0).sqrt() - 0.05;
    rough = rough.clamp(0.0, 1.0);

    let out_n = if smooth_n < 8 {
        rotate_vector_reverse_basis(
            Vec3::new(
                normal_grad_basis.x + nn1 * d_t2,
                normal_grad_basis.y + nn2 * d_t2,
                normal_grad_basis.z,
            ),
            right,
            up,
            forward,
        )
        .normalize()
    } else {
        normal
    };
    (out_n, rough)
}

/// CalcHSsoft port for directional lights, matching MB3D's packed soft-HS high bits.
fn calc_hs_soft_bits_mb3d(
    hit_pos: Vec3,
    depth_world: f64,
    ray_dir: Vec3,      // camera -> object direction
    normal: Vec3,       // world-space hit normal
    light_dir: Vec3,    // object -> light direction
    i_light_pos: u8,
    y: usize,
    formulas: &HybridProgram,
    params: &RenderParams,
) -> i32 {
    let view_dir = ray_dir.normalize();

    // Pre-refine HS start along the view direction so CalcHSsoft starts close to the same
    // DE-stop boundary used by the primary marcher.
    let mut refined_pos = hit_pos;
    let mut refined_depth = depth_world.max(0.0);
    let mut refine_step = params.step_width;
    for _ in 0..8 {
        let (_, de_ref) = calc_de(refined_pos, formulas, &params.iter_params, params.de_floor);
        let de_stop_ref = destop_at_steps(params, refined_depth / params.step_width);
        if de_ref <= de_stop_ref {
            refined_pos = refined_pos.add(view_dir.scale(-refine_step));
            refined_depth = (refined_depth - refine_step).max(0.0);
        } else {
            refined_pos = refined_pos.add(view_dir.scale(refine_step));
            refined_depth += refine_step;
        }
        refine_step *= 0.5;
    }

    // CalcPart shifts the HS start point by -0.1 march units before CalcHS/CalcHSsoft.
    let mut depth_steps = refined_depth / params.step_width - 0.1;
    if depth_steps < 0.0 {
        depth_steps = 0.0;
    }
    let mut pos = refined_pos.add(view_dir.scale(-0.1 * params.step_width));

    let zz = depth_steps.abs();
    let zend_steps = (params.max_ray_length / params.step_width).max(1.0e-30);
    let fov_y_rad = params.camera.fov_y * std::f64::consts::PI / 180.0;
    let max_l_hs = (params.camera.width as f64 + y as f64)
        * 0.6
        * (1.0
            + 0.5
                * zz.min(zend_steps * 0.4)
                * fov_y_rad.max(0.0)
                / (params.camera.height as f64).max(1.0))
        * params.hs_max_length_multiplier.max(1.0e-30);
    if max_l_hs <= 0.0 {
        return 63;
    }

    // Default state in CalcHSsoft: high bits prefilled with $FC00 (all light).
    let mut zr_soft = 1.0f64;

    let is_positional = (i_light_pos & 1) != 0;
    let zr_s_mul = if is_positional {
        if (i_light_pos & 6) == 2 {
            70.0 / params.soft_shadow_radius.max(1.0e-30)
        } else {
            40.0 / params.soft_shadow_radius.max(1.0e-30)
        }
    } else {
        80.0 / params.soft_shadow_radius.max(1.0e-30)
    };

    // Positional branch needs per-pixel world light position (PLValigned.LN + Xmit), which is not
    // stored in this renderer yet. Keep source-equivalent default high bits until that path is wired.
    if is_positional {
        return 63;
    }

    let n = normal.normalize();
    let l = light_dir.normalize();
    let v = view_dir;
    let hs_vec = l.scale(-1.0);
    let zz2mul = -hs_vec.dot(v); // == dot(light_dir, ray_dir)

    if n.dot(hs_vec) > 0.0 {
        return 0;
    }

    let mut d_t1 = max_l_hs;
    let mut zz2_steps = depth_steps;
    let mut ms_de_stop_world = destop_at_steps(params, zz2_steps);
    let mut step_factor_diff = 1.0f64;
    let mut de_world = calc_de(pos, formulas, &params.iter_params, params.de_floor).1;

    loop {
        let r_last_de_world = de_world;
        let max_step_world = (ms_de_stop_world.max(0.4 * params.step_width)) * params.mct_mh04_zsd;
        let r_last_step_world = ((de_world - params.ms_de_sub * ms_de_stop_world)
            * params.s_z_step_div_raw
            * step_factor_diff)
            .max(MB3D_MIN_STEP_UNITS * params.step_width)
            .min(max_step_world);
        if r_last_step_world <= 0.0 {
            break;
        }
        let r_last_step_width = r_last_step_world / params.step_width;
        d_t1 -= r_last_step_width;

        pos = pos.add(l.scale(r_last_step_world));
        zz2_steps += r_last_step_width * zz2mul;
        ms_de_stop_world = destop_at_steps(params, zz2_steps);

        let (iters, next_de_world) = calc_de(pos, formulas, &params.iter_params, params.de_floor);
        de_world = next_de_world;

        let traveled = (max_l_hs - d_t1).max(0.0);
        let soft_term = ((de_world - ms_de_stop_world) / params.step_width) * zr_s_mul / (traveled + MB3D_MIN_STEP_UNITS)
            + (traveled / max_l_hs.max(1.0e-30)).powi(8);
        zr_soft = zr_soft.min(soft_term);

        if iters >= params.iter_params.max_iters || de_world <= ms_de_stop_world {
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

/// Ray march a single pixel, matching MB3D's RayMarch procedure.
pub fn ray_march(
    origin: Vec3,
    dir: Vec3,
    formulas: &HybridProgram,
    params: &RenderParams,
    seed0: u32,
) -> PixelResult {
    let mut t = 0.0f64;
    let mut last_de: f64;
    let mut last_step: f64;
    let mut rsfmul: f64 = 1.0;
    let mut step_count = 0.0f64;
    let mut seed = seed0;
    let mut first_step = params.first_step_random;
    let de_floor = params.de_floor;
    let dfog_on_it = params.d_fog_on_it;

    // First evaluation at starting position
    let pos = origin.add(dir.scale(t));
    let (iters, de) = calc_de(pos, formulas, &params.iter_params, de_floor);

    // Check if already inside the set
    let current_destop = destop_at_steps(params, t / params.step_width);
    if iters >= params.iter_params.max_iters || de < current_destop {
        return PixelResult::Hit {
            depth: t,
            iters,
            shadow_steps: step_count.round().clamp(0.0, 1023.0) as i32,
        };
    }

    // Initialize last step from first DE
    last_step = (de * params.s_z_step_div).max(MB3D_MIN_STEP_UNITS * params.step_width);
    last_de = de;

    let max_steps = 2000000;
    for _ in 0..max_steps {
        let current_destop = destop_at_steps(params, t / params.step_width);

        // Evaluate DE
        let pos = origin.add(dir.scale(t));
        let (iters, mut de) = calc_de(pos, formulas, &params.iter_params, de_floor);

        // DE growth clamping: prevent jumps past features
        if de > last_de + last_step {
            de = last_de + last_step;
        }

        // Check if not hit — take next step
        if iters < params.iter_params.max_iters && de >= current_destop {
            // Source path: step from (DE - msDEsub*msDEstop), min floor, then clamp by dynamic max-step.
            let mut step = ((de - params.ms_de_sub * current_destop) * params.s_z_step_div * rsfmul)
                .max(MB3D_MIN_STEP_UNITS * params.step_width);
            let max_step_here = (current_destop.max(0.4 * params.step_width)) * params.mct_mh04_zsd;

            if max_step_here < step {
                if dfog_on_it == 0 || iters == dfog_on_it as i32 {
                    step_count += max_step_here / step;
                }
                step = max_step_here;
            } else if dfog_on_it == 0 || iters == dfog_on_it as i32 {
                step_count += 1.0;
            }

            if first_step {
                seed = seed.wrapping_mul(214013).wrapping_add(2531011);
                first_step = false;
                let jitter = ((seed & 0x7fff_ffff) as f64) * (1.0 / 2147483647.0);
                step *= jitter;
            }

            // Overshoot detection (RSFmul update)
            if last_de > de + 1e-30 {
                let ratio = last_step / (last_de - de);
                if ratio < 1.0 {
                    rsfmul = ratio.max(0.5);
                } else {
                    rsfmul = 1.0;
                }
            } else {
                rsfmul = 1.0;
            }

            last_de = de;
            last_step = step;
            t += step;

            if t > params.max_ray_length {
                return PixelResult::Miss;
            }
        } else {
            // ##### Surface found #####
            // Binary search refinement (MB3D's RMdoBinSearch)
            let mut refine_step = last_step * -0.5;
            for _ in 0..params.bin_search_steps {
                t += refine_step;
                let rpos = origin.add(dir.scale(t));
                let destop_here = destop_at_steps(params, t / params.step_width);
                let (ri, rd) = calc_de(rpos, formulas, &params.iter_params, de_floor);
                if rd < destop_here || ri >= params.iter_params.max_iters {
                    refine_step = -(refine_step.abs() * 0.55); // back up
                } else {
                    refine_step = refine_step.abs() * 0.55; // forward
                }
            }

            let hit_pos = origin.add(dir.scale(t));
            let (final_iters, _) = calc_de(hit_pos, formulas, &params.iter_params, de_floor);

            return PixelResult::Hit {
                depth: t,
                iters: final_iters,
                shadow_steps: step_count.round().clamp(0.0, 1023.0) as i32,
            };
        }
    }

    PixelResult::Miss
}

use std::thread;

/// Render the full image using two passes:
/// 1. Ray march to build depth + iteration buffers
/// 2. Compute normals and shade
pub fn render(formulas: &HybridProgram, params: &RenderParams, lighting: &crate::m3p::M3PLighting, ssao: &crate::m3p::M3PSSAO) -> Vec<u8> {
    let w = params.camera.width as usize;
    let h = params.camera.height as usize;
    if w == 0 || h == 0 {
        return Vec::new();
    }

    // Pass 1: build depth and iteration buffers
    let mut depth_buf = vec![f64::MAX; w * h];
    let mut iter_buf = vec![0i32; w * h];
    let mut shadow_buf = vec![0i32; w * h];

    eprintln!("Rendering {}x{} ...", w, h);
    let start = std::time::Instant::now();
    
    let num_threads = thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(h.max(1));
    let rows_per_thread = h.div_ceil(num_threads);
    let band_len = rows_per_thread * w;
    let ray_grid = build_ray_grid(&params.camera, num_threads, rows_per_thread);
    eprintln!("Using {} threads", num_threads);
    
    let total_hits = thread::scope(|s| {
        let mut workers = Vec::new();
        let ray_grid = &ray_grid;
        for (thread_idx, ((depth_chunk, iter_chunk), shadow_chunk)) in depth_buf
            .chunks_mut(band_len)
            .zip(iter_buf.chunks_mut(band_len))
            .zip(shadow_buf.chunks_mut(band_len))
            .enumerate()
        {
            let formulas = &formulas;
            let params = &params;
            let y_start = thread_idx * rows_per_thread;
            workers.push(s.spawn(move || {
                let row_count = depth_chunk.len() / w;
                let mut local_hits = 0u64;

                for local_y in 0..row_count {
                    let y = y_start + local_y;
                    let row_offset = local_y * w;
                    for x in 0..w {
                        let idx = row_offset + x;
                        let origin = ray_grid.row_origins[y].add(ray_grid.x_offsets[x]);
                        let dir = ray_grid.dirs[y * w + x];
                        let seed = (x as u32)
                            .wrapping_mul(0x45d9f3b)
                            .wrapping_add((y as u32).wrapping_mul(0x2710_0001))
                            .wrapping_add((thread_idx as u32).wrapping_mul(0x9e37_79b9))
                            .wrapping_add(0x2456_3487);

                        if let PixelResult::Hit {
                            depth,
                            iters,
                            shadow_steps,
                        } = ray_march(origin, dir, formulas, params, seed)
                        {
                            local_hits += 1;
                            depth_chunk[idx] = depth;
                            iter_chunk[idx] = iters;
                            shadow_chunk[idx] = shadow_steps;
                        }
                    }
                }

                local_hits
            }));
        }

        let mut total_hits = 0u64;
        for worker in workers {
            total_hits += worker.join().unwrap();
        }
        total_hits
    });

    eprintln!("Ray march complete in {:.1}s ({} hits / {} pixels)",
        start.elapsed().as_secs_f64(), total_hits, w * h);

    // Pass 2: compute normals and shade
    let mut pixels = vec![0u8; w * h * 4];
    let soft_hs_light = crate::lighting::soft_hs_light_dir(lighting, &params.camera, params);
    let lighting_cache = crate::lighting::LightingCache::new(lighting, &params.camera, params);

    thread::scope(|s| {
        let mut workers = Vec::new();

        for (band_idx, pixel_chunk) in pixels.chunks_mut(rows_per_thread * w * 4).enumerate() {
            let formulas = &formulas;
            let params = &params;
            let depth_buf = &depth_buf;
            let iter_buf = &iter_buf;
            let shadow_buf = &shadow_buf;
            let ray_grid = &ray_grid;
            let soft_hs_light = soft_hs_light;
            let lighting_cache = &lighting_cache;
            let y_start = band_idx * rows_per_thread;

            workers.push(s.spawn(move || {
                let mut shade_scratch = crate::lighting::ShadeScratch::default();
                let row_count = pixel_chunk.len() / (w * 4);
                for local_y in 0..row_count {
                    let y = y_start + local_y;
                    let row_offset = local_y * w * 4;

                    for x in 0..w {
                        let idx = y * w + x;
                        let offset = row_offset + x * 4;
                        let depth = depth_buf[idx];
                        let origin = ray_grid.row_origins[y].add(ray_grid.x_offsets[x]);

                        if depth == f64::MAX {
                            pixel_chunk[offset] = 10;
                            pixel_chunk[offset + 1] = 10;
                            pixel_chunk[offset + 2] = 15;
                            pixel_chunk[offset + 3] = 255;
                            continue;
                        }

                        let dir = ray_grid.dirs[idx];
                        let hit_pos = origin.add(dir.scale(depth));
                        let m_zz = depth / params.step_width;
                        let n_offset = mb3d_normal_offset(params, m_zz);

                        let (normal_basis, normal_coarse) = estimate_normal_grad(
                            hit_pos,
                            n_offset,
                            params.camera.forward,
                            params.camera.right,
                            params.camera.up,
                            formulas,
                            &params.iter_params,
                            params.de_floor,
                        );
                        let (normal_mb3d, roughness_mb3d) = smooth_normal_mb3d(
                            hit_pos,
                            normal_basis,
                            normal_coarse,
                            n_offset,
                            params.sm_normals,
                            params.camera.forward,
                            params.camera.right,
                            params.camera.up,
                            formulas,
                            &params.iter_params,
                            params.de_floor,
                        );

                        // Packed PsiLight.Shadow equivalent:
                        // low 10 bits = march/fog counter, high 6 bits = softHS factor.
                        let mut shadow_word = shadow_buf[idx] & 0x3ff;
                        if let Some((_li, light_dir, i_light_pos)) = soft_hs_light {
                            shadow_word |= 0xFC00;
                            let soft_bits = calc_hs_soft_bits_mb3d(
                                hit_pos,
                                depth,
                                dir,
                                normal_mb3d,
                                light_dir,
                                i_light_pos,
                                y,
                                formulas,
                                params,
                            );
                            shadow_word = (shadow_word & 0x03FF) | (soft_bits << 10);
                        }

                        let color = crate::lighting::shade(
                            normal_mb3d,
                            roughness_mb3d,
                            dir.scale(-1.0),
                            iter_buf[idx],
                            shadow_word,
                            params.iter_params.max_iters,
                            params.iter_params.min_iters,
                            hit_pos,
                            1.0,
                            depth,
                            x as i32,
                            y as i32,
                            (y as f64 + 0.5) / h as f64,
                            params.max_ray_length,
                            lighting_cache,
                            ssao,
                            formulas,
                            params,
                            &mut shade_scratch,
                        );

                        pixel_chunk[offset] = color[0];
                        pixel_chunk[offset + 1] = color[1];
                        pixel_chunk[offset + 2] = color[2];
                        pixel_chunk[offset + 3] = 255;
                    }
                }
            }));
        }

        for worker in workers {
            worker.join().unwrap();
        }
    });

    eprintln!("Render complete in {:.1}s", start.elapsed().as_secs_f64());
    pixels
}
