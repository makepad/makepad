use crate::formulas::{self, FormulaSlot, IterParams};

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
    pub fn mul(self, o: Self) -> Self { Vec3::new(self.x*o.x, self.y*o.y, self.z*o.z) }
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
    pub z1: f64,          // dZstart - dZmid
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
        let z1 = m3p.z_start - m3p.z_mid;

        eprintln!("  StepWidth: {:.6e}", step_width);
        eprintln!("  z_start (world): {:.10e}", z_start);
        eprintln!("  z1: {:.10e}", z1);
        eprintln!("  right:   ({:.6e}, {:.6e}, {:.6e})", right.x, right.y, right.z);
        eprintln!("  up:      ({:.6e}, {:.6e}, {:.6e})", up.x, up.y, up.z);
        eprintln!("  forward: ({:.6e}, {:.6e}, {:.6e})", forward.x, forward.y, forward.z);
        eprintln!("  mid:     ({:.10e}, {:.10e}, {:.10e})", mid.x, mid.y, mid.z);

        Camera {
            mid,
            right,
            up,
            forward,
            step_width,
            z_start,
            z_end: m3p.z_end,
            z1,
            width: m3p.width,
            height: m3p.height,
            fov_y: m3p.fov_y,
        }
    }

    /// Compute ray start position and direction for pixel (px, py).
    pub fn v_local_for_pixel(&self, px: i32, py: i32) -> Vec3 {
        let px_f = self.width as f64 * 0.5 - px as f64; // MB3D: FOVXoff - ix
        let py_f = py as f64 - self.height as f64 * 0.5; // MB3D: (y / iMandHeight - s05) * iMandHeight
        
        let fov_rad = self.fov_y * std::f64::consts::PI / 180.0;
        let fov_mul = fov_rad / self.height as f64;
        
        let cafx = px_f * fov_mul;
        let cafy = py_f * fov_mul;
        
        let (sin_x, cos_x) = cafx.sin_cos();
        let (sin_y, cos_y) = cafy.sin_cos();
        
        // BuildViewVectorDFOV: (-sinX, sinY, cosX*cosY)
        Vec3::new(-sin_x, sin_y, cos_x * cos_y).normalize()
    }

    pub fn ray_for_pixel(&self, px: i32, py: i32) -> (Vec3, Vec3) {
        let v_local = self.v_local_for_pixel(px, py);

        // In MB3D, the view vectors are scaled by StepWidth.
        // But we want a normalized direction.
        // Let's reconstruct the unscaled right/up/forward by dividing by StepWidth.
        let r = self.right.scale(1.0 / self.step_width);
        let u = self.up.scale(1.0 / self.step_width);
        let f = self.forward.scale(1.0 / self.step_width);

        // RotateVectorReverse
        let dir = r.scale(v_local.x).add(u.scale(v_local.y)).add(f.scale(v_local.z)).normalize();

        // In MB3D, for MCTCameraOptic = 0, the ray origin is calculated as:
        // C1 = Ystart + Vgrads[0]*ix + Vgrads[1]*iy
        // where Ystart = Mid + z1*Vgrads[2] - y1*Vgrads[1] - x1*Vgrads[0]
        // and z1 = (dZstart - dZmid) / StepWidth
        // Since Vgrads has length StepWidth:
        // origin = Mid + (dZstart - dZmid) * forward + (ix - width/2) * StepWidth * right + (iy - height/2) * StepWidth * up
        let cx = px as f64 - self.width as f64 * 0.5;
        let cy = py as f64 - self.height as f64 * 0.5;
        let start = self.mid
            .add(f.scale(self.z_start - self.mid.z))
            .add(r.scale(cx * self.step_width))
            .add(u.scale(cy * self.step_width));

        (start, dir)
    }
}

pub struct RenderParams {
    pub camera: Camera,
    pub iter_params: IterParams,
    pub max_ray_length: f64,   // maximum ray length in world units
    pub de_stop: f64,          // DE threshold for surface hit (world units, adjusted by de_scale)
    pub s_z_step_div: f64,     // effective step multiplier: sZstepDiv * de_scale
    pub s_z_step_div_raw: f64,
    pub b_dfog_it: u8,
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
    pub max_step: f64,         // maximum step size (world units)
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
    
    // First, determine DEoption exactly like MB3D's CheckFormulaOptions
    let mut de_option = -1;
    for i in 0..=end_to.min(5) {
        let f = &addon.formulas[i];
        if f.iteration_count == 0 { continue; }
        
        let de = de_option;
        // iDEoption for Amazing Box (4) is 11. For MengerIFS (20) it is 2.
        let f_de = match f.formula_nr {
            4 => 11,
            20 => 2,
            _ => 0, // default fallback
        };
        
        de_option = match f_de {
            -1 | 21 | 22 => de,
            2 => if [2, 5, 6, 11].contains(&de) { f_de } else { 0 },
            4 => if [5, 6].contains(&de) { f_de } else { 0 },
            5 => if de == 4 { 4 } else if [2, 11].contains(&de) { 2 } else if de != 6 { 0 } else { f_de },
            6 => if [2, 4, 5, 11].contains(&de) { f_de } else { 0 },
            11 => if [2, 5].contains(&de) { 2 } else if de != 6 { 0 } else { f_de },
            _ => f_de,
        };
    }
    if de_option > 19 {
        de_option = -1;
    }

    let is_custom_de = [2, 5, 6, 11].contains(&de_option);
    if !is_custom_de {
        return 1.0;
    }

    // Since our DE formula (r / |dr|) returns distance in world units,
    // and we don't scale our ray direction by StepWidth,
    // we don't need to divide the DE scale by StepWidth.
    // We can just return 1.0.
    1.0
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

        let z_corr = (fov_y_rad.max(1.0) / m3p.height as f64).sin();
        let z_cmul = 32767.0 * 256.0 / ((((m3p.z_end - m3p.z_start) / m3p.step_width) * z_corr + 1.0).sqrt() - 0.999999999);

        // Maximum step size (in world units)
        // MB3D: max_step = max(DEstop, 0.4) * mctMH04ZSD * StepWidth
        let mct_mh04_zsd = m3p.width.max(m3p.height) as f64;
        let max_step = de_stop_header.max(0.4) * mct_mh04_zsd * step_width;

        // DE floor: min 0.25 * effective DEstop (in world units)
        let de_floor = de_stop * 0.25;

        // Binary search steps
        let bin_search_steps = 6;

        eprintln!("  max_ray_length: {:.6e}", max_ray_length);
        eprintln!("  de_stop_world: {:.6e} ({:.4} step units)", de_stop_world, de_stop_header);
        eprintln!("  de_scale: {:.4} (dDEscale_computed)", de_scale);
        eprintln!("  effective de_stop: {:.6e}", de_stop);
        eprintln!("  effective sZstepDiv: {:.4} (raw {:.4} * de_scale {:.4})", s_z_step_div, s_z_step_div_raw, de_scale);
        eprintln!("  msDEsub: {:.4}", ms_de_sub);
        eprintln!("  max_step: {:.6e}", max_step);
        eprintln!("  de_floor: {:.6e}", de_floor);
        eprintln!("  iSmNormals: {}", ((m3p.i_options >> 6) & 0x0F));

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
                repeat_from: (m3p.addon.b_hyb_opt1 >> 4) as usize,
            },
            max_ray_length,
            de_stop,
            s_z_step_div,
            s_z_step_div_raw,
            step_width,
            de_stop_factor,
            b_dfog_it: m3p.b_dfog_it,
            b_vol_light_nr: m3p.b_vol_light_nr,
            b_calculate_hard_shadow: m3p.b_calculate_hard_shadow,
            b_hs_calculated: m3p.b_hs_calculated,
            b_calc1_hs_soft: m3p.b_calc1_hs_soft,
            soft_shadow_radius: m3p.soft_shadow_radius.max(0.001),
            hs_max_length_multiplier: m3p.hs_max_length_multiplier.max(0.001),
            ms_de_sub,
            max_step,
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

/// Compute distance estimation at a point, with DE floor clamping.
/// MB3D's CalcDEanalytic: if DE < DEstop * 0.25 then DE = DEstop * 0.25
fn calc_de(pos: Vec3, formulas: &[FormulaSlot], params: &IterParams, de_floor: f64) -> (i32, f64) {
    let (iters, de) = formulas::hybrid_de((pos.x, pos.y, pos.z), formulas, params);
    (iters, de.max(de_floor))
}

/// Estimate raw normal gradient via 6-point central differences along view vectors.
/// Matches MB3D's RMCalculateNormals sampling pattern.
fn estimate_normal_grad(pos: Vec3, eps: f64, forward: Vec3, right: Vec3, up: Vec3,
                        formulas: &[FormulaSlot], params: &IterParams, de_floor: f64) -> Vec3 {
    let fwd = forward.normalize().scale(eps);
    let rt = right.normalize().scale(eps);
    let upv = up.normalize().scale(eps);

    // Central differences measured in camera basis directions.
    let dz = calc_de(pos.add(fwd), formulas, params, de_floor).1
           - calc_de(pos.sub(fwd), formulas, params, de_floor).1;
    let dx = calc_de(pos.add(rt), formulas, params, de_floor).1
           - calc_de(pos.sub(rt), formulas, params, de_floor).1;
    let dy = calc_de(pos.add(upv), formulas, params, de_floor).1
           - calc_de(pos.sub(upv), formulas, params, de_floor).1;

    // Recompose back to world-space gradient.
    rt.normalize()
        .scale(dx)
        .add(upv.normalize().scale(dy))
        .add(fwd.normalize().scale(dz))
}

/// MB3D RMCalculateNormals probe offset:
/// Noffset = min(1, DEstop) * (1 + abs(mZZ) * mctDEstopFactor) * 0.15 * StepWidth.
fn mb3d_normal_offset(params: &RenderParams, m_zz: f64) -> f64 {
    params.de_stop_header.min(1.0)
        * (1.0 + m_zz.abs() * params.de_stop_factor)
        * 0.15
        * params.step_width
}

fn tangent_basis(normal: Vec3) -> (Vec3, Vec3) {
    let n = normal.normalize();
    let seed = if n.z.abs() < 0.9 {
        Vec3::new(0.0, 0.0, 1.0)
    } else {
        Vec3::new(0.0, 1.0, 0.0)
    };
    let mut vx = seed.cross(n).normalize();
    if vx.len() < 1e-10 {
        vx = Vec3::new(1.0, 0.0, 0.0).cross(n).normalize();
    }
    let vy = n.cross(vx).normalize();
    (vx, vy)
}

fn smooth_normal_mb3d(
    pos: Vec3,
    normal_grad: Vec3,
    n_offset: f64,
    smooth_n: i32,
    right: Vec3,
    up: Vec3,
    formulas: &[FormulaSlot],
    params: &IterParams,
    de_floor: f64,
) -> (Vec3, f64) {
    let normal = normal_grad.normalize();
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
        // RMCalculateNormals: after doubling Noffset, smooth midpoint uses +/-X and +/-Y probes.
        dnn = (
            dnn
            + calc_de(pos.add(right.normalize().scale(-noffset2)), formulas, params, de_floor).1
            + calc_de(pos.add(right.normalize().scale(noffset2)), formulas, params, de_floor).1
            + calc_de(pos.add(up.normalize().scale(-noffset2)), formulas, params, de_floor).1
            + calc_de(pos.add(up.normalize().scale(noffset2)), formulas, params, de_floor).1
        ) * 0.2;
    }

    let (vx, vy) = tangent_basis(normal);
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
    let denom = 1.0e-40 + normal_grad.dot(normal_grad);
    let mut rough = ((d_sg * 7.0 * d_t2 * d_t2) / denom).max(0.0).sqrt() - 0.05;
    rough = rough.clamp(0.0, 1.0);

    let out_n = if smooth_n < 8 {
        normal_grad
            .add(vx.scale(nn1 * d_t2))
            .add(vy.scale(nn2 * d_t2))
            .normalize()
    } else {
        normal
    };
    (out_n, rough)
}

/// Lightweight DE-based soft shadow toward a directional light.
fn soft_shadow(
    pos: Vec3,
    light_dir: Vec3,
    formulas: &[FormulaSlot],
    params: &RenderParams,
    steps: i32,
    max_dist: f64,
) -> f64 {
    if steps <= 0 {
        return 1.0;
    }
    let mut t = params.de_stop * 8.0;
    let mut res = 1.0f64;
    for _ in 0..steps {
        if t >= max_dist {
            break;
        }
        let p = pos.add(light_dir.scale(t));
        let (_, de) = calc_de(p, formulas, &params.iter_params, params.de_floor);
        let h = de.max(params.de_floor);
        let penumbra = (8.0 * h / t.max(1e-20)).clamp(0.0, 1.0);
        res = res.min(penumbra);
        t += h.max(params.de_stop * 3.0);
        if res < 0.05 {
            break;
        }
    }
    res.clamp(0.0, 1.0)
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
    formulas: &[FormulaSlot],
    params: &RenderParams,
) -> i32 {
    // CalcPart shifts the HS start point by -0.1 march units before CalcHS/CalcHSsoft.
    let mut depth_steps = depth_world / params.step_width - 0.1;
    if depth_steps < 0.0 {
        depth_steps = 0.0;
    }
    let mut pos = hit_pos.add(ray_dir.normalize().scale(-0.1 * params.step_width));

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
    let v = ray_dir.normalize();
    let hs_vec = l.scale(-1.0);
    let zz2mul = -hs_vec.dot(v); // == dot(light_dir, ray_dir)

    if n.dot(hs_vec) > 0.0 {
        return 0;
    }

    let mut d_t1 = max_l_hs;
    let mut zz2_world = depth_steps * params.step_width;
    let mut ms_de_stop_world = params.de_stop * (1.0 + zz2_world * params.de_stop_factor);
    let mut step_factor_diff = 2.0f64;
    let (mut iters, mut de_world) = calc_de(pos, formulas, &params.iter_params, params.de_floor);
    let mut de_steps = de_world / params.step_width;

    loop {
        let r_last_de_world = de_world;
        let r_last_step_world = ((de_world - params.ms_de_sub * ms_de_stop_world)
            * params.s_z_step_div_raw
            * step_factor_diff)
            .max(0.011 * params.step_width)
            .min(params.max_step);
        if r_last_step_world <= 0.0 {
            break;
        }
        let r_last_step_width = r_last_step_world / params.step_width;
        d_t1 -= r_last_step_width;

        pos = pos.add(l.scale(r_last_step_world));
        zz2_world += r_last_step_world * zz2mul;
        ms_de_stop_world = params.de_stop * (1.0 + zz2_world * params.de_stop_factor);

        (iters, de_world) = calc_de(pos, formulas, &params.iter_params, params.de_floor);
        de_steps = de_world / params.step_width;

        let traveled = (max_l_hs - d_t1).max(0.0);
        let soft_term = ((de_world - ms_de_stop_world) / params.step_width) * zr_s_mul / (traveled + 0.011)
            + (traveled / max_l_hs.max(1.0e-30)).powi(8);
        zr_soft = zr_soft.min(soft_term);

        if iters >= params.iter_params.max_iters || de_world <= ms_de_stop_world {
            break;
        }
        if de_world > r_last_de_world + r_last_step_world {
            de_world = r_last_de_world + r_last_step_world;
            de_steps = de_world / params.step_width;
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
pub fn ray_march(origin: Vec3, dir: Vec3, formulas: &[FormulaSlot], params: &RenderParams) -> PixelResult {
    let mut t = 0.0f64;
    let mut last_de = f64::MAX;
    let mut last_step = 0.0f64;
    let mut rsfmul: f64 = 1.0;
    let mut steps_taken = 0i32;
    let de_stop = params.de_stop;
    let de_floor = params.de_floor;

    // First evaluation at starting position
    let pos = origin.add(dir.scale(t));
    let (iters, de) = calc_de(pos, formulas, &params.iter_params, de_floor);

    // Check if already inside the set
    let current_destop = de_stop * (1.0 + t * params.de_stop_factor);
    if iters >= params.iter_params.max_iters || de < current_destop {
        return PixelResult::Hit { depth: t, iters, shadow_steps: steps_taken };
    }

    // Initialize last step from first DE
    last_step = de * params.s_z_step_div;
    last_de = de;

    let max_steps = 2000000;
    for _ in 0..max_steps {
        let current_destop = de_stop * (1.0 + t * params.de_stop_factor);

        // Evaluate DE
        let pos = origin.add(dir.scale(t));
        let (iters, mut de) = calc_de(pos, formulas, &params.iter_params, de_floor);

        // DE growth clamping: prevent jumps past features
        if de > last_de + last_step {
            de = last_de + last_step;
        }

        // Check if not hit — take next step
        if iters < params.iter_params.max_iters && de >= current_destop {
            // Base-correct primary march path (matches prior working render geometry).
            let mut step = de * params.s_z_step_div * rsfmul;
            if step > params.max_step {
                step = params.max_step;
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
            steps_taken += 1;

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
                let destop_here = de_stop * (1.0 + t * params.de_stop_factor);
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
                shadow_steps: steps_taken,
            };
        }
    }

    PixelResult::Miss
}

use std::sync::{Arc, Mutex};
use std::thread;

/// Render the full image using two passes:
/// 1. Ray march to build depth + iteration buffers
/// 2. Compute normals and shade
pub fn render(formulas: &[FormulaSlot], params: &RenderParams, lighting: &crate::m3p::M3PLighting, ssao: &crate::m3p::M3PSSAO) -> Vec<u8> {
    let w = params.camera.width as usize;
    let h = params.camera.height as usize;

    // Pass 1: build depth and iteration buffers
    let mut depth_buf = vec![f64::MAX; w * h];
    let mut iter_buf = vec![0i32; w * h];
    let mut shadow_buf = vec![0i32; w * h];

    eprintln!("Rendering {}x{} ...", w, h);
    let start = std::time::Instant::now();
    
    let num_threads = thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    eprintln!("Using {} threads", num_threads);
    
    let next_y = Arc::new(Mutex::new(0usize));
    let hit_count = Arc::new(Mutex::new(0u64));
    
    // We need to share formulas and params across threads.
    // Since they are not easily Send+Sync without Arc, and we don't want to change their signatures,
    // we'll use scoped threads if possible, or just Arc.
    // Actually, std::thread::scope is available in Rust 1.63+
    
    thread::scope(|s| {
        let mut threads = Vec::new();
        
        // We need to write to depth_buf and iter_buf. We can divide the buffers into chunks,
        // or use a mutex. Mutex per pixel is slow.
        // Let's divide rows among threads.
        // We can create a channel or just divide the rows upfront.
        
        for thread_idx in 0..num_threads {
            let formulas = &formulas;
            let params = &params;
            let next_y = Arc::clone(&next_y);
            let hit_count = Arc::clone(&hit_count);
            
            // To avoid unsafe, we can return the results from the thread and merge them.
            threads.push(s.spawn(move || {
                let mut local_results = Vec::new();
                let mut local_hits = 0u64;
                
                loop {
                    let y = {
                        let mut ny = next_y.lock().unwrap();
                        let current = *ny;
                        if current >= h {
                            break;
                        }
                        *ny += 1;
                        current
                    };
                    
                    if thread_idx == 0 && y % (h.max(100) / 20).max(1) == 0 {
                        let pct = y * 100 / h;
                        eprintln!("  {}% ({:.1}s)", pct, start.elapsed().as_secs_f64());
                    }
                    
                    let mut row_depth = vec![f64::MAX; w];
                    let mut row_iter = vec![0i32; w];
                    let mut row_shadow = vec![0i32; w];
                    
                    for x in 0..w {
                        let (origin, dir) = params.camera.ray_for_pixel(x as i32, y as i32);
                        let result = ray_march(origin, dir, formulas, params);
                        
                        match result {
                            PixelResult::Hit { depth, iters, shadow_steps } => {
                                local_hits += 1;
                                row_depth[x] = depth;
                                row_iter[x] = iters;
                                row_shadow[x] = shadow_steps;
                            }
                            PixelResult::Miss => {}
                        }
                    }
                    
                    local_results.push((y, row_depth, row_iter, row_shadow));
                }
                
                let mut hc = hit_count.lock().unwrap();
                *hc += local_hits;
                
                local_results
            }));
        }
        
        for t in threads {
            let results = t.join().unwrap();
            for (y, row_depth, row_iter, row_shadow) in results {
                let offset = y * w;
                depth_buf[offset..offset + w].copy_from_slice(&row_depth);
                iter_buf[offset..offset + w].copy_from_slice(&row_iter);
                shadow_buf[offset..offset + w].copy_from_slice(&row_shadow);
            }
        }
    });

    let total_hits = *hit_count.lock().unwrap();
    eprintln!("Ray march complete in {:.1}s ({} hits / {} pixels)",
        start.elapsed().as_secs_f64(), total_hits, w * h);

    // Optional iteration post-smoothing for experiments.
    // Keep disabled by default for source-faithful behavior.
    let use_iter_median = std::env::var("ITER_MEDIAN")
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);
    let smooth_iter = if use_iter_median {
        smooth_iter_buf(&iter_buf, w, h)
    } else {
        iter_buf.clone()
    };

    // Pass 3: compute normals and shade
    let mut pixels = vec![0u8; w * h * 4];
    let debug_mode = std::env::var("DEBUG_MODE").unwrap_or_default();

    eprintln!("  Iter median filter: {}", if use_iter_median { "on" } else { "off" });
    let detail_normal_mix = std::env::var("DETAIL_NORMAL_MIX").ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    let shadow_steps = std::env::var("SHADOW_STEPS").ok()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(48);
    let shadow_strength = std::env::var("SHADOW_STRENGTH").ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0);
    let simple_deao = std::env::var("SIMPLE_DEAO")
        .ok()
        .map(|v| v == "1")
        .unwrap_or(false);
    let strict_mb3d = true;
    let shadow_light_dir = crate::lighting::dominant_shadow_light_dir(lighting, &params.camera);
    let soft_hs_light = crate::lighting::soft_hs_light_dir(lighting, &params.camera, params);

    thread::scope(|s| {
        let mut threads = Vec::new();
        let next_y = Arc::new(Mutex::new(0usize));
        
        for _ in 0..num_threads {
            let formulas = &formulas;
            let params = &params;
            let depth_buf = &depth_buf;
            let smooth_iter = &smooth_iter;
            let shadow_buf = &shadow_buf;
            let debug_mode = &debug_mode;
            let next_y = Arc::clone(&next_y);
            let shadow_light_dir = shadow_light_dir;
            let soft_hs_light = soft_hs_light;
            let detail_normal_mix = detail_normal_mix;
            let simple_deao = simple_deao;
            
            threads.push(s.spawn(move || {
                let mut local_pixels = Vec::new();
                
                loop {
                    let y = {
                        let mut ny = next_y.lock().unwrap();
                        let current = *ny;
                        if current >= h {
                            break;
                        }
                        *ny += 1;
                        current
                    };
                    
                    let mut row_pixels = vec![0u8; w * 4];
                    
                    for x in 0..w {
                        let idx = y * w + x;
                        let offset = x * 4;
                        let d = depth_buf[idx];

                        if d == f64::MAX {
                            row_pixels[offset] = 10;
                            row_pixels[offset + 1] = 10;
                            row_pixels[offset + 2] = 15;
                            row_pixels[offset + 3] = 255;
                            continue;
                        }

                        let color = match debug_mode.as_str() {
                            "depth" => {
                                let t = (d / params.max_ray_length).clamp(0.0, 1.0);
                                let v = ((1.0 - t) * 255.0) as u8;
                                [v, v, v]
                            }
                            "iters" => {
                                let t = (smooth_iter[idx] as f64) / (params.iter_params.max_iters as f64);
                                let (r, g, b) = crate::lighting::iteration_color(t, lighting);
                                [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8]
                            }
                            "normals" => {
                                // Visualize the same MB3D-inspired normal path used for shading.
                                let (origin, dir) = params.camera.ray_for_pixel(x as i32, y as i32);
                                let hit_pos = origin.add(dir.scale(d));
                                let m_zz = d / params.step_width;
                                let n_offset = mb3d_normal_offset(params, m_zz);
                                let normal_coarse = estimate_normal_grad(
                                    hit_pos,
                                    n_offset,
                                    params.camera.forward, params.camera.right, params.camera.up,
                                    formulas, &params.iter_params, params.de_floor,
                                );
                                let (normal, _roughness) = smooth_normal_mb3d(
                                    hit_pos,
                                    normal_coarse,
                                    n_offset,
                                    params.sm_normals,
                                    params.camera.right,
                                    params.camera.up,
                                    formulas,
                                    &params.iter_params,
                                    params.de_floor,
                                );
                                [
                                    ((normal.x * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8,
                                    ((normal.y * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8,
                                    ((normal.z * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8,
                                ]
                            }
                            "ssn" => {
                                let normal = screen_space_normal(&depth_buf, x, y, w, h, params.step_width);
                                let (origin, dir) = params.camera.ray_for_pixel(x as i32, y as i32);
                                let hit_pos = origin.add(dir.scale(d));
                                crate::lighting::shade(
                                    normal,
                                    0.0,
                                    dir.scale(-1.0),
                                    smooth_iter[idx],
                                    shadow_buf[idx],
                                    params.iter_params.max_iters,
                                    params.iter_params.min_iters,
                                    hit_pos,
                                    &params.camera,
                                    1.0,
                                    1.0,
                                    d,
                                    (y as f64 + 0.5) / h as f64,
                                    params.max_ray_length,
                                    lighting,
                                    ssao,
                                    formulas,
                                    params,
                                    x == w / 2 && y == h / 2,
                                )
                            }
                            _ => {
                                // DE-based normals along view vectors
                                let (origin, dir) = params.camera.ray_for_pixel(x as i32, y as i32);
                                let hit_pos = origin.add(dir.scale(d));
                                
                                let m_zz = d / params.step_width;
                                let n_offset = mb3d_normal_offset(params, m_zz);
                                
                                let normal_coarse = estimate_normal_grad(
                                    hit_pos, n_offset,
                                    params.camera.forward, params.camera.right, params.camera.up,
                                    formulas, &params.iter_params, params.de_floor,
                                );
                                let (normal_mb3d, roughness_mb3d) = smooth_normal_mb3d(
                                    hit_pos,
                                    normal_coarse,
                                    n_offset,
                                    params.sm_normals,
                                    params.camera.right,
                                    params.camera.up,
                                    formulas,
                                    &params.iter_params,
                                    params.de_floor,
                                );
                                let normal_screen = screen_space_normal(&depth_buf, x, y, w, h, params.step_width);
                                let normal_shade = normal_mb3d
                                    .scale(1.0 - detail_normal_mix)
                                    .add(normal_screen.scale(detail_normal_mix))
                                    .normalize();
                                
                                // Source-faithful default: AO is handled in lighting::shade (ray-based path).
                                // Keep old simple DEAO as opt-in experiment.
                                let ao_factor = if simple_deao {
                                    let mut deao = 0.0;
                                    let ao_base = params.de_stop * 2.0;
                                    for i in 1..=5 {
                                        let step_dist = ao_base * (i as f64);
                                        let ao_pos = hit_pos.add(normal_shade.scale(step_dist));
                                        let (_, de) = calc_de(ao_pos, formulas, &params.iter_params, params.de_floor);
                                        deao += (step_dist - de).max(0.0) / step_dist;
                                    }
                                    let mut d_amb_s = (1.0 - deao / 5.0 * 2.0).clamp(0.0, 1.0);
                                    let s_amplitude = ssao.amb_shad;
                                    if s_amplitude > 1.0 {
                                        d_amb_s = d_amb_s + (s_amplitude - 1.0) * (d_amb_s * d_amb_s - d_amb_s);
                                    } else {
                                        d_amb_s = 1.0 - s_amplitude * (1.0 - d_amb_s);
                                    }
                                    d_amb_s
                                } else {
                                    1.0
                                };
                                let direct_light_factor = if strict_mb3d {
                                    1.0
                                } else if let Some(light_dir) = shadow_light_dir {
                                    let shadow_raw = soft_shadow(
                                        hit_pos.add(normal_shade.scale(params.de_stop * 4.0)),
                                        light_dir,
                                        formulas,
                                        params,
                                        shadow_steps,
                                        params.max_ray_length * 0.25,
                                    );
                                    1.0 - shadow_strength * (1.0 - shadow_raw)
                                } else {
                                    1.0
                                };

                                let is_center = x == w / 2 && y == h / 2;

                                // Packed PsiLight.Shadow equivalent:
                                // low 10 bits = march/fog counter, high 6 bits = softHS factor.
                                let mut shadow_word = shadow_buf[idx] & 0x3ff;
                                if strict_mb3d {
                                    if let Some((_li, light_dir, i_light_pos)) = soft_hs_light {
                                        shadow_word |= 0xFC00;
                                        let soft_bits = calc_hs_soft_bits_mb3d(
                                            hit_pos,
                                            d,
                                            dir,
                                            normal_shade,
                                            light_dir,
                                            i_light_pos,
                                            y,
                                            formulas,
                                            params,
                                        );
                                        shadow_word = (shadow_word & 0x03FF) | (soft_bits << 10);
                                    }
                                }

                                crate::lighting::shade(
                                    normal_shade,
                                    roughness_mb3d,
                                    dir.scale(-1.0),
                                    smooth_iter[idx],
                                    shadow_word,
                                    params.iter_params.max_iters,
                                    params.iter_params.min_iters,
                                    hit_pos,
                                    &params.camera,
                                    ao_factor,
                                    direct_light_factor,
                                    d,
                                    (y as f64 + 0.5) / h as f64,
                                    params.max_ray_length,
                                    lighting,
                                    ssao,
                                    formulas,
                                    params,
                                    is_center,
                                )
                            }
                        };
                        row_pixels[offset] = color[0];
                        row_pixels[offset + 1] = color[1];
                        row_pixels[offset + 2] = color[2];
                        row_pixels[offset + 3] = 255;
                    }
                    local_pixels.push((y, row_pixels));
                }
                local_pixels
            }));
        }
        
        for t in threads {
            let results = t.join().unwrap();
            for (y, row_pixels) in results {
                let offset = y * w * 4;
                pixels[offset..offset + w * 4].copy_from_slice(&row_pixels);
            }
        }
    });

    eprintln!("Render complete in {:.1}s", start.elapsed().as_secs_f64());
    pixels
}

/// Smooth iteration buffer using 5x5 median filter to reduce speckle noise.
fn smooth_iter_buf(iter_buf: &[i32], w: usize, h: usize) -> Vec<i32> {
    let mut out = vec![0i32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut samples = Vec::with_capacity(25);
            for dy in 0..5i32 {
                for dx in 0..5i32 {
                    let nx = x as i32 + dx - 2;
                    let ny = y as i32 + dy - 2;
                    if nx >= 0 && nx < w as i32 && ny >= 0 && ny < h as i32 {
                        samples.push(iter_buf[ny as usize * w + nx as usize]);
                    }
                }
            }
            samples.sort_unstable();
            out[y * w + x] = samples[samples.len() / 2]; // median
        }
    }
    out
}

/// Compute screen-space normal from depth buffer gradients.
fn screen_space_normal(depth_buf: &[f64], x: usize, y: usize, w: usize, h: usize, step_width: f64) -> Vec3 {
    let d = depth_buf[y * w + x];
    let miss = f64::MAX;

    let dl = if x > 0 { depth_buf[y * w + (x - 1)] } else { miss };
    let dr = if x + 1 < w { depth_buf[y * w + (x + 1)] } else { miss };
    let du = if y > 0 { depth_buf[(y - 1) * w + x] } else { miss };
    let dd = if y + 1 < h { depth_buf[(y + 1) * w + x] } else { miss };

    let dx = if dl != miss && dr != miss {
        (dr - dl) * 0.5
    } else if dr != miss {
        dr - d
    } else if dl != miss {
        d - dl
    } else {
        0.0
    };

    let dy = if du != miss && dd != miss {
        (dd - du) * 0.5
    } else if dd != miss {
        dd - d
    } else if du != miss {
        d - du
    } else {
        0.0
    };

    // Normal from depth gradient: depth differences are in world units,
    // pixel spacing is StepWidth, so z-component = StepWidth for proper scale
    Vec3::new(-dx, -dy, step_width).normalize()
}
