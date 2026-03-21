use makepad_widgets::*;

use crate::audio::get_audio_state;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    set_type_default() do #(DrawVisualizer::script_shader(vm)){
        ..mod.draw.DrawQuad
        time: 0.0
        amplitude: 0.0
        mode: 0.0
        b0: 0.0  b1: 0.0  b2: 0.0  b3: 0.0
        b4: 0.0  b5: 0.0  b6: 0.0  b7: 0.0
        b8: 0.0  b9: 0.0  b10: 0.0 b11: 0.0
        b12: 0.0 b13: 0.0 b14: 0.0 b15: 0.0

        pixel: fn() {
            return vec4(0.0, 0.0, 0.0, 1.0)
        }
    }

    mod.widgets.VisualizerBase = #(Visualizer::register_widget(vm))

    mod.widgets.Visualizer = set_type_default() do mod.widgets.VisualizerBase{
        width: Fill
        height: Fill
    }

    // Built-in: Spectrum Bars
    mod.widgets.SpectrumBars = set_type_default() do mod.widgets.VisualizerBase{
        width: Fill height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let band_w = 1.0 / 16.0
                let band_idx = floor(uv.x / band_w)
                let band_local = fract(uv.x / band_w)
                let mut val = 0.0
                if band_idx < 1.0 { val = self.b0 }
                else if band_idx < 2.0 { val = self.b1 }
                else if band_idx < 3.0 { val = self.b2 }
                else if band_idx < 4.0 { val = self.b3 }
                else if band_idx < 5.0 { val = self.b4 }
                else if band_idx < 6.0 { val = self.b5 }
                else if band_idx < 7.0 { val = self.b6 }
                else if band_idx < 8.0 { val = self.b7 }
                else if band_idx < 9.0 { val = self.b8 }
                else if band_idx < 10.0 { val = self.b9 }
                else if band_idx < 11.0 { val = self.b10 }
                else if band_idx < 12.0 { val = self.b11 }
                else if band_idx < 13.0 { val = self.b12 }
                else if band_idx < 14.0 { val = self.b13 }
                else if band_idx < 15.0 { val = self.b14 }
                else { val = self.b15 }
                let bar_h = val * 0.85
                let in_bar = step(1.0 - uv.y, bar_h) * step(0.08, band_local) * step(band_local, 0.92)
                let hue = uv.x * 0.7
                let s_r = 0.5 + 0.5 * cos(6.28318 * (hue + 0.0))
                let s_g = 0.5 + 0.5 * cos(6.28318 * (hue + 0.33))
                let s_b = 0.5 + 0.5 * cos(6.28318 * (hue + 0.67))
                let glow = exp(-abs(1.0 - uv.y - bar_h) * 12.0) * val * 0.7
                let bar_color = vec3(s_r * 0.9, s_g * 0.9, s_b * 0.9) * in_bar
                let glow_color = vec3(s_r, s_g, s_b) * glow
                let final_color = bar_color + glow_color
                let alpha = max(in_bar, glow * 0.8)
                return vec4(final_color * alpha, alpha)
            }
        }
    }

    // Built-in: Circular Spectrum
    mod.widgets.SpectrumCircular = set_type_default() do mod.widgets.VisualizerBase{
        width: Fill height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos * 2.0 - vec2(1.0, 1.0)
                let aspect = self.rect_size.x / max(self.rect_size.y, 0.001)
                let p = vec2(uv.x * aspect, uv.y)
                let angle = atan2(p.y, p.x)
                let radius = length(p)
                let t = self.time
                let norm_angle = (angle + 3.14159) / 6.28318
                let band_idx = floor(norm_angle * 16.0)
                let mut val = 0.0
                if band_idx < 1.0 { val = self.b0 }
                else if band_idx < 2.0 { val = self.b1 }
                else if band_idx < 3.0 { val = self.b2 }
                else if band_idx < 4.0 { val = self.b3 }
                else if band_idx < 5.0 { val = self.b4 }
                else if band_idx < 6.0 { val = self.b5 }
                else if band_idx < 7.0 { val = self.b6 }
                else if band_idx < 8.0 { val = self.b7 }
                else if band_idx < 9.0 { val = self.b8 }
                else if band_idx < 10.0 { val = self.b9 }
                else if band_idx < 11.0 { val = self.b10 }
                else if band_idx < 12.0 { val = self.b11 }
                else if band_idx < 13.0 { val = self.b12 }
                else if band_idx < 14.0 { val = self.b13 }
                else if band_idx < 15.0 { val = self.b14 }
                else { val = self.b15 }
                let ring = 0.3 + val * 0.35
                let dist = abs(radius - ring)
                let alpha = 1.0 - smoothstep(0.005, 0.035, dist)
                let hue = norm_angle + t * 0.05
                let c_r = 0.5 + 0.5 * cos(6.28318 * (hue + 0.0))
                let c_g = 0.5 + 0.5 * cos(6.28318 * (hue + 0.33))
                let c_b = 0.5 + 0.5 * cos(6.28318 * (hue + 0.67))
                let inner_glow = exp(-radius * 3.0) * self.amplitude * 0.4
                let bg = vec3(0.02, 0.02, 0.06) + vec3(inner_glow * 0.3, inner_glow * 0.1, inner_glow * 0.5)
                let ring_color = vec3(c_r, c_g, c_b) * alpha
                let final_color = bg * (1.0 - alpha) + ring_color
                return vec4(final_color, 1.0)
            }
        }
    }
}

// DrawVisualizer -- instance vars for shader
#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawVisualizer {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    time: f32,
    #[live]
    amplitude: f32,
    #[live]
    mode: f32,
    #[live] b0: f32, #[live] b1: f32, #[live] b2: f32, #[live] b3: f32,
    #[live] b4: f32, #[live] b5: f32, #[live] b6: f32, #[live] b7: f32,
    #[live] b8: f32, #[live] b9: f32, #[live] b10: f32, #[live] b11: f32,
    #[live] b12: f32, #[live] b13: f32, #[live] b14: f32, #[live] b15: f32,
}

// Visualizer Widget
#[derive(Script, ScriptHook, Widget)]
pub struct Visualizer {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bg: DrawVisualizer,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    frame_started: bool,
    #[rust]
    area: Area,
    #[live(true)]
    pub visible: bool,
}

impl Widget for Visualizer {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                if !self.visible {
                    self.next_frame = cx.new_next_frame();
                    return;
                }
                self.draw_bg.time = ne.time as f32;
                let state = get_audio_state();
                self.draw_bg.amplitude = state.amplitude.get() as f32;
                if let Ok(bands) = state.spectrum.lock() {
                    self.draw_bg.b0 = bands[0]; self.draw_bg.b1 = bands[1];
                    self.draw_bg.b2 = bands[2]; self.draw_bg.b3 = bands[3];
                    self.draw_bg.b4 = bands[4]; self.draw_bg.b5 = bands[5];
                    self.draw_bg.b6 = bands[6]; self.draw_bg.b7 = bands[7];
                    self.draw_bg.b8 = bands[8]; self.draw_bg.b9 = bands[9];
                    self.draw_bg.b10 = bands[10]; self.draw_bg.b11 = bands[11];
                    self.draw_bg.b12 = bands[12]; self.draw_bg.b13 = bands[13];
                    self.draw_bg.b14 = bands[14]; self.draw_bg.b15 = bands[15];
                }
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
            self.frame_started = true;
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible { return DrawStep::done(); }
        // Ensure next_frame is running (may have missed Startup if created dynamically)
        if !self.frame_started {
            self.next_frame = cx.new_next_frame();
            self.frame_started = true;
        }
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}
