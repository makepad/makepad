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

#[derive(Debug, Clone)]
pub struct IterParams {
    pub max_iters: i32,
    pub min_iters: i32,
    pub rstop: f64,
    pub is_julia: bool,
    pub julia_x: f64,
    pub julia_y: f64,
    pub julia_z: f64,
    pub repeat_from: usize,
}

/// Amazing Box (Mandelbox) formula
/// Box-fold + sphere-fold + scale
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
        state.x = (state.x + f).abs() - (state.x - f).abs() - state.x;
        state.y = (state.y + f).abs() - (state.y - f).abs() - state.y;
        state.z = (state.z + f).abs() - (state.z - f).abs() - state.z;

        // Sphere fold using the packed constants from FillCustomVBufWithVars:
        // [PVar-24] = scale / min_r^2, [PVar-32] = min_r^2
        let rr = state.x * state.x + state.y * state.y + state.z * state.z;
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
        state.x = state.x * m + state.cx;
        state.y = state.y * m + state.cy;
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
        state.x = state.x.abs();
        state.y = state.y.abs();
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
pub enum FormulaKind {
    AmazingBox(AmazingBox),
    MengerIFS(MengerIFS),
}

pub struct FormulaSlot {
    pub kind: FormulaKind,
    pub iteration_count: i32,
}

impl FormulaSlot {
    pub fn iterate(&self, state: &mut IterState) {
        match &self.kind {
            FormulaKind::AmazingBox(f) => f.iterate(state),
            FormulaKind::MengerIFS(f) => f.iterate(state),
        }
    }
}

/// Run hybrid iteration (alternating mode) and compute distance estimation
pub fn hybrid_de(pos: (f64, f64, f64), formulas: &[FormulaSlot], params: &IterParams) -> (i32, f64) {
    let mut state = IterState::new(pos.0, pos.1, pos.2, params);
    // MB3D doHybridPasDE initializes w := 1 for the common AmBox + IFS path.
    state.w = 1.0;

    let mut total_iters = 0i32;
    let mut current_formula = 0;
    let mut current_iters_left = if formulas.is_empty() { 0 } else { formulas[0].iteration_count };

    'outer: loop {
        if current_iters_left <= 0 {
            current_formula += 1;
            if current_formula >= formulas.len() {
                // In MB3D, it repeats from `RepeatFrom`
                // But we didn't pass `RepeatFrom` to `hybrid_de`.
                // Let's pass it or store it in `formulas`.
                // Actually, we can just use the last formula if `formulas` only contains the ones up to `EndTo`.
                // Wait, `formulas` contains all formulas up to `EndTo`.
                // But we need `RepeatFrom`.
                // For now, let's just repeat the last formula if `RepeatFrom` is not available,
                // or let's add `repeat_from` to `IterParams`!
                current_formula = params.repeat_from;
            }
            current_iters_left = formulas[current_formula].iteration_count;
        }

        let slot = &formulas[current_formula];
        slot.iterate(&mut state);

        total_iters += 1;
        current_iters_left -= 1;
        state.r2 = state.x * state.x + state.y * state.y + state.z * state.z;

        if state.r2 > state.rstop || total_iters >= state.max_iters {
            break 'outer;
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
pub fn build_formulas(m3p: &crate::m3p::M3PFile) -> Vec<FormulaSlot> {
    let addon = &m3p.addon;
    let mut slots = Vec::new();

    // MB3D uses bHybOpt1 to determine the sequence
    // bHybOpt1 & 7 is the end index (inclusive)
    // bHybOpt1 >> 4 is the repeat index
    let end_to = (addon.b_hyb_opt1 & 7) as usize;
    let _repeat_from = (addon.b_hyb_opt1 >> 4) as usize;

    for i in 0..=end_to.min(5) {
        let f = &addon.formulas[i];
        if f.iteration_count <= 0 {
            continue;
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

    slots
}
