/// Paint types for vector drawing - solid colors and gradients.
/// Gradients are evaluated per-vertex on the CPU; the GPU interpolates
/// the resulting colors across triangles automatically.

#[derive(Clone, Debug)]
pub struct GradientStop {
    pub offset: f32,     // 0.0 .. 1.0
    pub color: [f32; 4], // RGBA premultiplied
}

#[derive(Clone, Debug)]
pub enum VectorPaint {
    Solid {
        color: [f32; 4],
    },
    LinearGradient {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        stops: Vec<GradientStop>,
    },
    RadialGradient {
        cx: f32,
        cy: f32,
        r: f32,
        stops: Vec<GradientStop>,
    },
}

impl VectorPaint {
    pub fn solid(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self::Solid {
            color: [r * a, g * a, b * a, a],
        }
    }

    pub fn from_hex(hex: u32, alpha: f32) -> Self {
        let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
        let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
        let b = (hex & 0xFF) as f32 / 255.0;
        Self::solid(r, g, b, alpha)
    }

    pub fn linear_gradient(x0: f32, y0: f32, x1: f32, y1: f32, stops: Vec<GradientStop>) -> Self {
        Self::LinearGradient {
            x0,
            y0,
            x1,
            y1,
            stops,
        }
    }

    pub fn radial_gradient(cx: f32, cy: f32, r: f32, stops: Vec<GradientStop>) -> Self {
        Self::RadialGradient { cx, cy, r, stops }
    }

    /// Evaluate paint color at a world-space position
    pub fn color_at(&self, x: f32, y: f32) -> [f32; 4] {
        match self {
            Self::Solid { color } => *color,
            Self::LinearGradient {
                x0,
                y0,
                x1,
                y1,
                stops,
            } => {
                let dx = x1 - x0;
                let dy = y1 - y0;
                let len2 = dx * dx + dy * dy;
                let t = if len2 > 1e-6 {
                    (((x - x0) * dx + (y - y0) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                sample_stops(stops, t)
            }
            Self::RadialGradient { cx, cy, r, stops } => {
                let dx = x - cx;
                let dy = y - cy;
                let d = (dx * dx + dy * dy).sqrt();
                let t = if *r > 1e-6 {
                    (d / r).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                sample_stops(stops, t)
            }
        }
    }
}

impl Default for VectorPaint {
    fn default() -> Self {
        Self::solid(1.0, 1.0, 1.0, 1.0)
    }
}

impl GradientStop {
    pub fn new(offset: f32, r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            offset,
            color: [r * a, g * a, b * a, a],
        }
    }

    pub fn from_hex(offset: f32, hex: u32, alpha: f32) -> Self {
        let r = ((hex >> 16) & 0xFF) as f32 / 255.0;
        let g = ((hex >> 8) & 0xFF) as f32 / 255.0;
        let b = (hex & 0xFF) as f32 / 255.0;
        Self::new(offset, r, g, b, alpha)
    }
}

fn sample_stops(stops: &[GradientStop], t: f32) -> [f32; 4] {
    if stops.is_empty() {
        return [1.0, 1.0, 1.0, 1.0];
    }
    if stops.len() == 1 || t <= stops[0].offset {
        return stops[0].color;
    }
    if t >= stops[stops.len() - 1].offset {
        return stops[stops.len() - 1].color;
    }
    for i in 1..stops.len() {
        if t <= stops[i].offset {
            let s0 = &stops[i - 1];
            let s1 = &stops[i];
            let range = s1.offset - s0.offset;
            let f = if range > 1e-6 {
                (t - s0.offset) / range
            } else {
                0.0
            };
            return [
                s0.color[0] + (s1.color[0] - s0.color[0]) * f,
                s0.color[1] + (s1.color[1] - s0.color[1]) * f,
                s0.color[2] + (s1.color[2] - s0.color[2]) * f,
                s0.color[3] + (s1.color[3] - s0.color[3]) * f,
            ];
        }
    }
    stops[stops.len() - 1].color
}
