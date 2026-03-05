#[cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

/// Iteration state for fractal computation
#[derive(Debug, Clone)]
pub struct IterState {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
    pub cx: f64,
    pub cy: f64,
    pub cz: f64,
    pub r2: f64,      // squared radius
    pub iters: i32,   // iteration count reached
    pub max_iters: i32,
    pub rstop: f64,   // bailout radius squared
}

impl IterState {
    pub fn new(x: f64, y: f64, z: f64, params: &IterParams) -> Self {
        IterState {
            x, y, z, w: 0.0,
            cx: if params.is_julia { params.julia_x } else { x },
            cy: if params.is_julia { params.julia_y } else { y },
            cz: if params.is_julia { params.julia_z } else { z },
            r2: 0.0,
            iters: 0,
            max_iters: params.max_iters,
            rstop: params.rstop,  // MB3D compares r² against RStop directly (not RStop²)
        }
    }
}

#[inline(always)]
fn simd_abs_xy(x: f64, y: f64) -> (f64, f64) {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let xy = vld1q_f64([x, y].as_ptr());
            let abs_xy = vabsq_f64(xy);
            let mut out = [0.0; 2];
            vst1q_f64(out.as_mut_ptr(), abs_xy);
            (out[0], out[1])
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            let xy = _mm_set_pd(y, x);
            let mask = _mm_castsi128_pd(_mm_set1_epi64x(i64::MAX));
            let abs_xy = _mm_and_pd(xy, mask);
            let mut out = [0.0; 2];
            _mm_storeu_pd(out.as_mut_ptr(), abs_xy);
            (out[0], out[1])
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        (x.abs(), y.abs())
    }
}

#[inline(always)]
fn simd_box_fold_xy(x: f64, y: f64, fold: f64) -> (f64, f64) {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let xy = vld1q_f64([x, y].as_ptr());
            let ff = vdupq_n_f64(fold);
            let folded =
                vsubq_f64(vsubq_f64(vabsq_f64(vaddq_f64(xy, ff)), vabsq_f64(vsubq_f64(xy, ff))), xy);
            let mut out = [0.0; 2];
            vst1q_f64(out.as_mut_ptr(), folded);
            (out[0], out[1])
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            let xy = _mm_set_pd(y, x);
            let ff = _mm_set1_pd(fold);
            let mask = _mm_castsi128_pd(_mm_set1_epi64x(i64::MAX));
            let plus_abs = _mm_and_pd(_mm_add_pd(xy, ff), mask);
            let minus_abs = _mm_and_pd(_mm_sub_pd(xy, ff), mask);
            let folded = _mm_sub_pd(_mm_sub_pd(plus_abs, minus_abs), xy);
            let mut out = [0.0; 2];
            _mm_storeu_pd(out.as_mut_ptr(), folded);
            (out[0], out[1])
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        (
            (x + fold).abs() - (x - fold).abs() - x,
            (y + fold).abs() - (y - fold).abs() - y,
        )
    }
}

#[inline(always)]
fn simd_dot2(x: f64, y: f64) -> f64 {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let xy = vld1q_f64([x, y].as_ptr());
            vaddvq_f64(vmulq_f64(xy, xy))
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            let xy = _mm_set_pd(y, x);
            let sq = _mm_mul_pd(xy, xy);
            let hi = _mm_unpackhi_pd(sq, sq);
            _mm_cvtsd_f64(_mm_add_sd(sq, hi))
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        x * x + y * y
    }
}

#[inline(always)]
fn simd_mul_add_xy(x: f64, y: f64, mul: f64, add_x: f64, add_y: f64) -> (f64, f64) {
    #[cfg(target_arch = "aarch64")]
    {
        unsafe {
            let xy = vld1q_f64([x, y].as_ptr());
            let mul_xy = vmulq_n_f64(xy, mul);
            let add_xy = vld1q_f64([add_x, add_y].as_ptr());
            let out_xy = vaddq_f64(mul_xy, add_xy);
            let mut out = [0.0; 2];
            vst1q_f64(out.as_mut_ptr(), out_xy);
            (out[0], out[1])
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        unsafe {
            let xy = _mm_set_pd(y, x);
            let mm = _mm_set1_pd(mul);
            let add_xy = _mm_set_pd(add_y, add_x);
            let out_xy = _mm_add_pd(_mm_mul_pd(xy, mm), add_xy);
            let mut out = [0.0; 2];
            _mm_storeu_pd(out.as_mut_ptr(), out_xy);
            (out[0], out[1])
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        (x * mul + add_x, y * mul + add_y)
    }
}

#[derive(Debug, Clone)]
pub struct IterParams {
    pub max_iters: i32,
    pub min_iters: i32,
    pub rstop: f64,
    pub is_julia: bool,
    pub julia_x: f64,
    pub julia_y: f64,
    pub julia_z: f64,
}

/// Amazing Box (Mandelbox) formula
/// Box-fold + sphere-fold + scale
#[derive(Clone)]
pub struct AmazingBox {
    pub scale: f64,
    pub scale_div_min_r2: f64,
    pub min_r2: f64,
    pub fold: f64,
}

impl AmazingBox {
    pub fn new(scale: f64, min_r: f64, fold: f64) -> Self {
        let min_r = min_r.max(1.0e-40);
        let min_r2 = min_r * min_r;
        let scale_div_min_r2 = scale / min_r2;
        AmazingBox {
            scale,
            scale_div_min_r2,
            min_r2,
            fold,
        }
    }

    /// One iteration for MB3D's HybridCubeDE.
    pub fn iterate(&self, state: &mut IterState) {
        // Box fold: clamp each axis to [-fold, fold], then reflect
        // equivalent to: x = clamp(x, -fold, fold) * 2 - x
        let f = self.fold;
        let (folded_x, folded_y) = simd_box_fold_xy(state.x, state.y, f);
        state.x = folded_x;
        state.y = folded_y;
        state.z = (state.z + f).abs() - (state.z - f).abs() - state.z;

        // Sphere fold using the packed constants from FillCustomVBufWithVars:
        // [PVar-24] = scale / min_r^2, [PVar-32] = min_r^2
        let rr = simd_dot2(state.x, state.y) + state.z * state.z;
        let m = if rr < self.min_r2 {
            self.scale_div_min_r2
        } else if rr < 1.0 {
            self.scale / rr
        } else {
            self.scale
        };

        // MB3D's hybrid DE uses w as the common DE accumulator.
        state.w *= m;

        // Scale and add constant
        let (next_x, next_y) = simd_mul_add_xy(state.x, state.y, m, state.cx, state.cy);
        state.x = next_x;
        state.y = next_y;
        state.z = state.z * m + state.cz;
    }
}

#[derive(Debug, Clone)]
pub struct Mat3 {
    pub m: [[f64; 3]; 3],
}

impl Mat3 {
    pub fn identity() -> Self {
        Mat3 {
            m: [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ]
        }
    }

    pub fn from_euler(xa: f64, ya: f64, za: f64) -> Self {
        let (sin_x, cos_x) = xa.sin_cos();
        let (sin_y, cos_y) = ya.sin_cos();
        let (sin_z, cos_z) = za.sin_cos();

        Mat3 {
            m: [
                [cos_y * cos_z, -cos_y * sin_z, sin_y],
                [sin_x * sin_y * cos_z + cos_x * sin_z, cos_x * cos_z - sin_x * sin_y * sin_z, -sin_x * cos_y],
                [sin_x * sin_z - cos_x * sin_y * cos_z, cos_x * sin_y * sin_z + sin_x * cos_z, cos_x * cos_y],
            ]
        }
    }

    pub fn transform(&self, x: f64, y: f64, z: f64) -> (f64, f64, f64) {
        // MB3D MengerIFS applies rotation as M * v
        // nx = M[0,0]*x + M[0,1]*y + M[0,2]*z
        (
            x * self.m[0][0] + y * self.m[0][1] + z * self.m[0][2],
            x * self.m[1][0] + y * self.m[1][1] + z * self.m[1][2],
            x * self.m[2][0] + y * self.m[2][1] + z * self.m[2][2],
        )
    }
}

/// MengerIFS formula - Menger sponge via Iterated Function System
#[derive(Clone)]
pub struct MengerIFS {
    pub scale: f64,
    pub cx: f64,
    pub cy: f64,
    pub cz: f64,
    pub rot: Mat3,
}

impl MengerIFS {
    pub fn new(scale: f64, cx: f64, cy: f64, cz: f64, rot: Mat3) -> Self {
        MengerIFS { scale, cx, cy, cz, rot }
    }

    /// One iteration for MB3D's hybrid IFS path.
    pub fn iterate(&self, state: &mut IterState) {
        // Fold: absolute value
        let (abs_x, abs_y) = simd_abs_xy(state.x, state.y);
        state.x = abs_x;
        state.y = abs_y;
        state.z = state.z.abs();

        // Sort axes (largest first) - creates octahedral symmetry
        if state.x < state.y {
            std::mem::swap(&mut state.x, &mut state.y);
        }
        if state.x < state.z {
            std::mem::swap(&mut state.x, &mut state.z);
        }
        if state.y < state.z {
            std::mem::swap(&mut state.y, &mut state.z);
        }

        // Apply rotation
        let (nx, ny, nz) = self.rot.transform(state.x, state.y, state.z);

        // Scale and translate
        let sf = self.scale - 1.0;
        state.x = self.scale * nx - self.cx * sf;
        state.y = self.scale * ny - self.cy * sf;
        
        // Z-fold: reflection across C applied to nz
        let z_scaled = self.scale * nz;
        let c = self.cz * sf;
        state.z = c - (z_scaled - c).abs();

        // w tracks cumulative scale for DE
        state.w *= self.scale;
    }
}

/// Formula slot in a hybrid setup
#[derive(Clone)]
pub enum FormulaKind {
    AmazingBox(AmazingBox),
    MengerIFS(MengerIFS),
}

pub struct FormulaSlot {
    pub kind: FormulaKind,
    pub iteration_count: i32,
}

impl FormulaSlot {
    #[inline(always)]
    pub fn iterate(&self, state: &mut IterState) {
        match &self.kind {
            FormulaKind::AmazingBox(f) => f.iterate(state),
            FormulaKind::MengerIFS(f) => f.iterate(state),
        }
    }
}

pub struct HybridProgram {
    cycle_ops: Box<[FormulaKind]>,
    repeat_from_op: usize,
}

impl HybridProgram {
    pub fn is_empty(&self) -> bool {
        self.cycle_ops.is_empty()
    }
}

/// Run hybrid iteration (alternating mode) and compute distance estimation
pub fn hybrid_de(pos: (f64, f64, f64), program: &HybridProgram, params: &IterParams) -> (i32, f64) {
    let mut state = IterState::new(pos.0, pos.1, pos.2, params);
    // MB3D doHybridPasDE initializes w := 1 for the common AmBox + IFS path.
    state.w = 1.0;
    if program.cycle_ops.is_empty() {
        return (0, 0.0);
    }

    let mut total_iters = 0i32;
    let mut op_idx = 0usize;

    'outer: loop {
        match &program.cycle_ops[op_idx] {
            FormulaKind::AmazingBox(f) => f.iterate(&mut state),
            FormulaKind::MengerIFS(f) => f.iterate(&mut state),
        }
        total_iters += 1;
        state.r2 = simd_dot2(state.x, state.y) + state.z * state.z;

        if state.r2 > state.rstop || total_iters >= state.max_iters {
            break 'outer;
        }

        op_idx += 1;
        if op_idx >= program.cycle_ops.len() {
            op_idx = program.repeat_from_op;
        }
    }

    state.iters = total_iters;

    let r = state.r2.sqrt();
    let de = if state.w.abs() > 1e-30 {
        r / state.w.abs()
    } else {
        0.0
    };

    (total_iters, de)
}

/// Build formula slots from M3P file data
pub fn build_formulas(m3p: &crate::m3p::M3PFile) -> HybridProgram {
    let addon = &m3p.addon;
    let mut slots = Vec::new();

    // MB3D uses bHybOpt1 to determine the sequence
    // bHybOpt1 & 7 is the end index (inclusive)
    // bHybOpt1 >> 4 is the repeat index
    let end_to = (addon.b_hyb_opt1 & 7) as usize;
    let repeat_from = (addon.b_hyb_opt1 >> 4) as usize;
    let mut repeat_from_slot = None;

    for i in 0..=end_to.min(5) {
        let f = &addon.formulas[i];
        if f.iteration_count <= 0 {
            continue;
        }
        if repeat_from_slot.is_none() && i >= repeat_from {
            repeat_from_slot = Some(slots.len());
        }

        let kind = match f.formula_nr {
            4 => {
                // Amazing Box (Mandelbox) built-in formula.
                // FillCustomVBufWithVars packs opt[1] type 7 as:
                // scale/min_r^2 and min_r^2, where min_r is clamped by Max(1e-40, raw).
                let scale = f.option_values[0];
                let min_r = f.option_values[1];
                let fold = f.option_values[2];
                FormulaKind::AmazingBox(AmazingBox::new(scale, min_r, fold))
            },
            _ => {
                // Check if it's a custom formula by name
                if f.custom_name.contains("Menger") || f.formula_nr == 20 {
                    // MengerIFS
                    let scale = if f.option_count > 0 { f.option_values[0] } else { 3.0 };
                    let cx = if f.option_count > 1 { f.option_values[1] } else { 1.0 };
                    let cy = if f.option_count > 2 { f.option_values[2] } else { 1.0 };
                    let cz = if f.option_count > 3 { f.option_values[3] } else { 0.5 };
                    
                    let rot_x = if f.option_count > 4 { f.option_values[4] } else { 0.0 };
                    let rot_y = if f.option_count > 5 { f.option_values[5] } else { 0.0 };
                    let rot_z = if f.option_count > 6 { f.option_values[6] } else { 0.0 };
                    
                    let rot = if rot_x == 0.0 && rot_y == 0.0 && rot_z == 0.0 {
                        Mat3::identity()
                    } else {
                        let d2r = std::f64::consts::PI / 180.0;
                        Mat3::from_euler(rot_x * d2r, rot_y * d2r, rot_z * d2r)
                    };
                    
                    FormulaKind::MengerIFS(MengerIFS::new(scale, cx, cy, cz, rot))
                } else {
                    eprintln!("Unknown formula #{}: '{}', skipping", f.formula_nr, f.custom_name);
                    continue;
                }
            }
        };

        slots.push(FormulaSlot {
            kind,
            iteration_count: f.iteration_count,
        });
    }

    let repeat_from_slot = repeat_from_slot.unwrap_or(0);
    let mut cycle_ops = Vec::new();
    let mut repeat_from_op = 0usize;
    for (slot_idx, slot) in slots.iter().enumerate() {
        if slot_idx == repeat_from_slot {
            repeat_from_op = cycle_ops.len();
        }
        cycle_ops.extend(std::iter::repeat_n(slot.kind.clone(), slot.iteration_count as usize));
    }

    let repeat_from_op = if cycle_ops.is_empty() {
        0
    } else {
        repeat_from_op.min(cycle_ops.len() - 1)
    };

    HybridProgram {
        cycle_ops: cycle_ops.into_boxed_slice(),
        repeat_from_op,
    }
}
