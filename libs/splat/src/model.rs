#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplatFileFormat {
    Ply,
    Sog,
}

#[derive(Clone, Debug)]
pub struct Splat {
    pub position: [f32; 3],
    pub scale: [f32; 3],
    pub rotation: [f32; 4], // xyzw
    pub color: [f32; 4],    // rgba in [0, 1]
}

#[derive(Clone, Debug)]
pub struct SplatHigherOrderSh {
    pub bands: usize,
    pub coeffs_per_channel: usize,
    pub coeffs: Vec<f32>, // packed as [splat][coeff][rgb]
}

impl SplatHigherOrderSh {
    pub fn coeffs_for_splat(&self, index: usize) -> Option<&[f32]> {
        let stride = self.coeffs_per_channel.checked_mul(3)?;
        let start = index.checked_mul(stride)?;
        let end = start.checked_add(stride)?;
        self.coeffs.get(start..end)
    }
}

#[derive(Clone, Debug)]
pub struct SplatScene {
    pub format: SplatFileFormat,
    pub splats: Vec<Splat>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub antialias: bool,
    pub higher_order_sh: Option<SplatHigherOrderSh>,
}

impl SplatScene {
    pub fn empty(format: SplatFileFormat) -> Self {
        Self {
            format,
            splats: Vec::new(),
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            antialias: false,
            higher_order_sh: None,
        }
    }

    pub fn recompute_bounds(&mut self) {
        if self.splats.is_empty() {
            self.bounds_min = [0.0; 3];
            self.bounds_max = [0.0; 3];
            return;
        }

        let mut min_v = [f32::INFINITY; 3];
        let mut max_v = [f32::NEG_INFINITY; 3];

        for splat in &self.splats {
            for axis in 0..3 {
                let value = splat.position[axis];
                min_v[axis] = min_v[axis].min(value);
                max_v[axis] = max_v[axis].max(value);
            }
        }

        self.bounds_min = min_v;
        self.bounds_max = max_v;
    }
}
