use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use super::scene_3d::Scene3D;

#[derive(Clone, Debug)]
pub struct Chart3DBar {
    pub x: usize,
    pub z: usize,
    pub value: f32,
    pub color: Vec4f,
}

#[derive(Clone, Debug)]
pub struct Chart3DData {
    pub bars: Vec<Chart3DBar>,
    pub x_bins: usize,
    pub z_bins: usize,
    pub max_value: f32,
    pub x_axis_label: String,
    pub z_axis_label: String,
    pub y_axis_label: String,
}

impl Default for Chart3DData {
    fn default() -> Self {
        Self::demo()
    }
}

impl Chart3DData {
    pub fn demo() -> Self {
        let x_bins = 12usize;
        let z_bins = 12usize;
        let mut bars = Vec::with_capacity(x_bins * z_bins);
        let mut max_value = 0.0_f32;

        for z in 0..z_bins {
            for x in 0..x_bins {
                let fx = if x_bins <= 1 {
                    0.0
                } else {
                    x as f32 / (x_bins as f32 - 1.0)
                };
                let fz = if z_bins <= 1 {
                    0.0
                } else {
                    z as f32 / (z_bins as f32 - 1.0)
                };

                let peak_a = (-((fx - 0.82).powi(2) * 28.0 + (fz - 0.74).powi(2) * 28.0)).exp();
                let peak_b = (-((fx - 0.58).powi(2) * 12.0 + (fz - 0.45).powi(2) * 10.0)).exp();
                let ridge = ((fx * 6.4).sin() * 0.5 + 0.5) * ((fz * 4.8).cos() * 0.5 + 0.5);
                let jitter_seed = (((x * 73 + z * 151 + 41) % 997) as f32) / 997.0;
                let noise = (jitter_seed - 0.5) * 0.08;

                let normalized =
                    (0.04 + 0.18 * ridge + 0.55 * peak_a + 0.35 * peak_b + noise).max(0.0);
                let value = (normalized * 1035.0).max(0.0);
                max_value = max_value.max(value);

                bars.push(Chart3DBar {
                    x,
                    z,
                    value,
                    color: vec4(0.0, 0.0, 0.0, 1.0),
                });
            }
        }

        let denom = max_value.max(0.0001);
        for bar in &mut bars {
            bar.color = Self::heatmap_color(bar.value / denom);
        }

        Self {
            bars,
            x_bins,
            z_bins,
            max_value,
            x_axis_label: "Laser Type".to_string(),
            z_axis_label: "Concentration (uM)".to_string(),
            y_axis_label: "Fluorescent Intensity".to_string(),
        }
    }

    fn heatmap_color(t: f32) -> Vec4f {
        let t = t.clamp(0.0, 1.0);
        let stops = [
            (0.00_f32, vec3(0.08, 0.07, 0.07)),
            (0.10_f32, vec3(0.26, 0.16, 0.10)),
            (0.22_f32, vec3(0.52, 0.24, 0.18)),
            (0.35_f32, vec3(0.82, 0.52, 0.12)),
            (0.48_f32, vec3(0.92, 0.88, 0.22)),
            (0.60_f32, vec3(0.56, 0.84, 0.60)),
            (0.72_f32, vec3(0.18, 0.52, 0.80)),
            (0.84_f32, vec3(0.16, 0.22, 0.70)),
            (0.94_f32, vec3(0.78, 0.54, 0.78)),
            (1.00_f32, vec3(0.92, 0.95, 0.96)),
        ];

        for i in 0..(stops.len() - 1) {
            let (a_t, a_c) = stops[i];
            let (b_t, b_c) = stops[i + 1];
            if t <= b_t {
                let span = (b_t - a_t).max(0.0001);
                let k = ((t - a_t) / span).clamp(0.0, 1.0);
                let c = a_c * (1.0 - k) + b_c * k;
                return vec4(c.x, c.y, c.z, 1.0);
            }
        }

        let (_, c) = stops[stops.len() - 1];
        vec4(c.x, c.y, c.z, 1.0)
    }
}

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.Chart3DBase = #(Chart3D::register_widget(vm))
    mod.widgets.Chart3D = set_type_default() do mod.widgets.Chart3DBase{
        width: Fill
        height: Fill
        draw_bg +: {
            color: #x171d26
            draw_depth: -99.0
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Chart3D {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[deref]
    scene_3d: Scene3D,
    #[rust]
    data: Chart3DData,
}

impl Chart3D {
    pub fn data(&self) -> &Chart3DData {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Chart3DData {
        &mut self.data
    }

    pub fn set_data(&mut self, data: Chart3DData) {
        self.data = data;
    }
}

impl Widget for Chart3D {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        let mut chart_scope = Scope::with_data(&mut self.data);
        self.scene_3d.handle_event(cx, event, &mut chart_scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let mut chart_scope = Scope::with_data(&mut self.data);
        self.scene_3d.draw_walk(cx, &mut chart_scope, walk)
    }
}
