use makepad_widgets::*;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::audio::{self, AudioPlaybackState};

// ============================================================================
// Demo song URLs (from makepad-component)
// ============================================================================

const SONGS: &[(&str, &str)] = &[
    (
        "Ambient Flow",
        "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-1.mp3",
    ),
    (
        "Electronic Pulse",
        "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-2.mp3",
    ),
    (
        "Synth Dream",
        "https://www.soundhelix.com/examples/mp3/SoundHelix-Song-3.mp3",
    ),
];

// ============================================================================
// Global shared state (set once at startup, used by Visualizer widgets)
// ============================================================================

fn get_audio_state() -> Arc<AudioPlaybackState> {
    use std::sync::OnceLock;
    static AUDIO_STATE: OnceLock<Arc<AudioPlaybackState>> = OnceLock::new();
    AUDIO_STATE
        .get_or_init(|| Arc::new(AudioPlaybackState::new()))
        .clone()
}

// ============================================================================
// script_mod! — Splash UI + 3 inline pixel shaders
// ============================================================================

script_mod! {
    use mod.prelude.widgets.*

    // === DrawVisualizer shader type ===
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

    let VisualizerBase = #(Visualizer::register_widget(vm))

    // ========================================================================
    // Style A: Spectrum Bars
    // ========================================================================
    let SpectrumBars = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
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

                let bg = vec3(0.02, 0.03, 0.08) + vec3(0.0, 0.0, 0.02) * (1.0 - uv.y)

                let bar_color = vec3(s_r * 0.9, s_g * 0.9, s_b * 0.9) * in_bar
                let glow_color = vec3(s_r, s_g, s_b) * glow
                let final_color = bg * (1.0 - max(in_bar, glow * 0.5)) + bar_color + glow_color

                return vec4(final_color, 1.0)
            }
        }
    }

    // ========================================================================
    // Style B: Circular Spectrum
    // ========================================================================
    let SpectrumCircular = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
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

    // ========================================================================
    // Style C: Wave + Particles
    // ========================================================================
    let SpectrumWave = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time

                let band_idx = floor(uv.x * 16.0)
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

                let wave_y = 0.5 + val * 0.3 * sin(uv.x * 25.0 + t * 3.0)
                let dist = abs(uv.y - wave_y)
                let line = 1.0 - smoothstep(0.0, 0.012, dist)

                let wave_y2 = 0.5 + val * 0.15 * sin(uv.x * 50.0 - t * 2.0 + 1.5)
                let dist2 = abs(uv.y - wave_y2)
                let line2 = (1.0 - smoothstep(0.0, 0.008, dist2)) * 0.5

                let gx = floor(uv.x * 40.0)
                let gy = floor(uv.y * 40.0)
                let seed = gx * 127.1 + gy * 311.7 + t * 0.3
                let r = fract(sin(seed) * 43758.5453)
                let particle = step(0.96 - self.amplitude * 0.15, r) * step(abs(uv.y - wave_y), 0.18)

                let hue = uv.x + t * 0.1
                let c_r = 0.5 + 0.5 * cos(6.28318 * (hue + 0.0))
                let c_g = 0.5 + 0.5 * cos(6.28318 * (hue + 0.33))
                let c_b = 0.5 + 0.5 * cos(6.28318 * (hue + 0.67))

                let bg = vec3(0.03, 0.02, 0.06)
                let alpha = max(max(line, line2), particle * 0.6)
                let final_color = bg * (1.0 - alpha) + vec3(c_r, c_g, c_b) * alpha

                return vec4(final_color, 1.0)
            }
        }
    }

    // ========================================================================
    // Style D: Fire Turbulence (Xor technique)
    // ========================================================================
    let SpectrumFire = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time

                // Average amplitude from bands for fire intensity
                let avg = (self.b0 + self.b1 + self.b2 + self.b3 + self.b4 + self.b5 + self.b6 + self.b7) / 8.0
                let intensity = max(avg, 0.05)

                // Turbulence: layered sine waves with rotation (Xor technique)
                let mut p = vec2((uv.x - 0.5) * 2.0, (1.0 - uv.y) * 2.5)
                // Scroll upward
                p.y = p.y - t * 1.5

                // Rotation matrix (not 45 or 90 degrees)
                let c_a = 0.6
                let s_a = 0.8
                let mut freq = 2.0
                let mut px = p.x
                let mut py = p.y

                // Turbulence octaves (unrolled for Splash shader)
                let mut turb = 0.0
                // Octave 1
                let phase1 = freq * (px * s_a + py * c_a) + t * 2.0
                px = px + 0.7 * c_a * sin(phase1) / freq
                py = py + 0.7 * s_a * sin(phase1) / freq
                let rx1 = px * c_a - py * s_a
                let ry1 = px * s_a + py * c_a
                px = rx1
                py = ry1
                freq = freq * 1.4
                // Octave 2
                let phase2 = freq * (px * s_a + py * c_a) + t * 2.5
                px = px + 0.7 * c_a * sin(phase2) / freq
                py = py + 0.7 * s_a * sin(phase2) / freq
                let rx2 = px * c_a - py * s_a
                let ry2 = px * s_a + py * c_a
                px = rx2
                py = ry2
                freq = freq * 1.4
                // Octave 3
                let phase3 = freq * (px * s_a + py * c_a) + t * 3.0
                px = px + 0.7 * c_a * sin(phase3) / freq
                py = py + 0.7 * s_a * sin(phase3) / freq
                let rx3 = px * c_a - py * s_a
                let ry3 = px * s_a + py * c_a
                px = rx3
                py = ry3
                freq = freq * 1.4
                // Octave 4
                let phase4 = freq * (px * s_a + py * c_a) + t * 3.5
                px = px + 0.7 * c_a * sin(phase4) / freq
                freq = freq * 1.4
                // Octave 5
                let phase5 = freq * (px * s_a + py * c_a) + t * 4.0
                turb = sin(phase1) * 0.5 + sin(phase2) * 0.3 + sin(phase3) * 0.2 + sin(phase4) * 0.15 + sin(phase5) * 0.1

                // Fire shape: fade out at edges and top
                let shape = (1.0 - uv.y) * smoothstep(0.0, 0.3, 0.5 - abs(uv.x - 0.5))
                let fire = clamp(shape + turb * 0.4 * intensity, 0.0, 1.0)

                // Fire color ramp: black → red → orange → yellow → white
                let fire_r = smoothstep(0.1, 0.5, fire)
                let fire_g = smoothstep(0.3, 0.8, fire) * 0.7
                let fire_b = smoothstep(0.6, 1.0, fire) * 0.3

                // Audio-reactive: bass boosts red, treble boosts blue tips
                let bass = (self.b0 + self.b1 + self.b2) / 3.0
                let treble = (self.b12 + self.b13 + self.b14 + self.b15) / 4.0
                let final_r = fire_r + bass * 0.3
                let final_g = fire_g * (1.0 + intensity * 0.5)
                let final_b = fire_b + treble * 0.4

                // Soft tonemapping (tanh approximation: x/(1+|x|))
                let tm_in = vec3(final_r, final_g, final_b) * 1.5
                let color = vec3(
                    tm_in.x / (1.0 + abs(tm_in.x)),
                    tm_in.y / (1.0 + abs(tm_in.y)),
                    tm_in.z / (1.0 + abs(tm_in.z))
                )

                return vec4(color, 1.0)
            }
        }
    }

    // ========================================================================
    // Style E: Galaxy (Xor Efficient Chaos + spiral arms)
    // ========================================================================
    let SpectrumStars = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time
                let aspect = self.rect_size.x / max(self.rect_size.y, 0.001)

                // Center coordinates
                let mut cx = (uv.x - 0.5) * aspect * 8.0
                let mut cy = (uv.y - 0.5) * 8.0

                // Slow galaxy rotation
                let rot_a = t * 0.03
                let ca = cos(rot_a)
                let sa = sin(rot_a)
                let tcx = cx * ca - cy * sa
                let tcy = cx * sa + cy * ca
                cx = tcx
                cy = tcy

                // Audio
                let bass = (self.b0 + self.b1 + self.b2 + self.b3) / 4.0
                let mid = (self.b6 + self.b7 + self.b8 + self.b9) / 4.0
                let treble = (self.b12 + self.b13 + self.b14 + self.b15) / 4.0

                // Polar coordinates for spiral arms
                let r = length(vec2(cx, cy))
                // Manual atan2(cy, cx): atan(y/x) with quadrant correction
                let abs_cx = max(abs(cx), 0.0001)
                let angle_raw = atan(cy / abs_cx)
                // When cx < 0, add/subtract PI based on sign of cy
                let angle = angle_raw + step(cx, 0.0) * (step(0.0, cy) * 2.0 - 1.0) * 3.14159

                // Spiral arm pattern: 2 arms with logarithmic spiral
                let arm_twist = 0.8
                let arm1 = sin(angle * 2.0 - r * arm_twist + t * 0.2) * 0.5 + 0.5
                let arm2 = sin(angle * 2.0 - r * arm_twist + 3.14159 + t * 0.2) * 0.5 + 0.5
                let arm_density = max(arm1, arm2)
                // Sharpen arms
                let arm = smoothstep(0.3, 0.9, arm_density)

                // Galaxy disc falloff (exponential)
                let disc = exp(-r * 0.3) * (1.0 + bass * 0.5)

                // Bright galactic core
                let core_glow = exp(-r * r * 0.8) * (1.2 + bass * 0.8)

                // === Efficient Chaos stars (4 layers, golden angle rotation) ===
                let gr_c = 0.22252093
                let gr_s = 0.97492791

                // Layer 1: main star field
                let mut px = cx * 1.5
                let mut py = cy * 1.5
                px = px + 1.309
                py = py + 1.309
                px = px + 0.15 * sin(py)
                py = py + 0.15 * sin(px)
                let mx1 = (px - 2.0 * floor(px / 2.0)) - 1.0
                let my1 = (py - 2.0 * floor(py / 2.0)) - 1.0
                let len1 = max(length(vec2(mx1, my1)), 0.001)
                let att1 = max(1.0 - len1, 0.0) / len1
                let rx1 = px * gr_c - py * gr_s
                let ry1 = px * gr_s + py * gr_c

                // Layer 2: medium stars
                px = rx1 + 2.618 * 0.3
                py = ry1 + 2.618 * 0.3
                px = px / (1.0 + 0.6 * 0.3)
                py = py / (1.0 + 0.6 * 0.3)
                px = px + 0.15 * sin(py)
                py = py + 0.15 * sin(px)
                let mx2 = (px - 2.0 * floor(px / 2.0)) - 1.0
                let my2 = (py - 2.0 * floor(py / 2.0)) - 1.0
                let len2 = max(length(vec2(mx2, my2)), 0.001)
                let att2 = max(1.0 - len2, 0.0) / len2
                let rx2 = px * gr_c - py * gr_s
                let ry2 = px * gr_s + py * gr_c

                // Layer 3: faint stars
                px = rx2 + 2.618 * 0.5
                py = ry2 + 2.618 * 0.5
                px = px / (1.0 + 0.6 * 0.5)
                py = py / (1.0 + 0.6 * 0.5)
                px = px + 0.15 * sin(py)
                py = py + 0.15 * sin(px)
                let mx3 = (px - 2.0 * floor(px / 2.0)) - 1.0
                let my3 = (py - 2.0 * floor(py / 2.0)) - 1.0
                let len3 = max(length(vec2(mx3, my3)), 0.001)
                let att3 = max(1.0 - len3, 0.0) / len3
                let rx3 = px * gr_c - py * gr_s
                let ry3 = px * gr_s + py * gr_c

                // Layer 4: dim stars
                px = rx3 + 2.618 * 0.7
                py = ry3 + 2.618 * 0.7
                px = px / (1.0 + 0.6 * 0.7)
                py = py / (1.0 + 0.6 * 0.7)
                let mx4 = (px - 2.0 * floor(px / 2.0)) - 1.0
                let my4 = (py - 2.0 * floor(py / 2.0)) - 1.0
                let len4 = max(length(vec2(mx4, my4)), 0.001)
                let att4 = max(1.0 - len4, 0.0) / len4

                // Stars concentrated along spiral arms
                let star_total = (att1 + att2 + att3 + att4) * (0.3 + arm * 0.7)
                let star_brightness = 0.015 + self.amplitude * 0.04 + treble * 0.02

                // Star colors: white/blue tint, warmer near core
                let star_r = star_total * (1.0 + bass * 1.5)
                let star_g = star_total * (0.9 + mid * 1.0)
                let star_b = star_total * (1.3 + treble * 2.0)

                // Nebula dust in spiral arms (warm reddish-purple)
                let dust_r = arm * disc * 0.15 * (1.0 + bass * 0.5)
                let dust_g = arm * disc * 0.05
                let dust_b = arm * disc * 0.12 * (1.0 + treble * 0.3)

                // Core color: warm yellow-white
                let core_r = core_glow * 1.0
                let core_g = core_glow * 0.85
                let core_b = core_glow * 0.5

                // Combine
                let final_r = star_r * star_brightness + dust_r + core_r
                let final_g = star_g * star_brightness + dust_g + core_g
                let final_b = star_b * star_brightness + dust_b + core_b

                // Dark space background with subtle nebula
                let nebula = sin(uv.x * 3.0 + t * 0.1) * sin(uv.y * 2.5 - t * 0.08) * 0.5 + 0.5
                let bg_r = 0.005 + nebula * 0.01
                let bg_g = 0.003 + nebula * 0.005
                let bg_b = 0.01 + nebula * 0.02

                // Tonemapping
                let tm_r = final_r + bg_r
                let tm_g = final_g + bg_g
                let tm_b = final_b + bg_b
                let color = vec3(
                    tm_r / (1.0 + abs(tm_r)),
                    tm_g / (1.0 + abs(tm_g)),
                    tm_b / (1.0 + abs(tm_b))
                )

                return vec4(color, 1.0)
            }
        }
    }

    // ========================================================================
    // Style F: Ice Crystal
    // ========================================================================
    let SpectrumIce = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time
                let aspect = self.rect_size.x / max(self.rect_size.y, 0.001)
                let px = (uv.x - 0.5) * aspect * 6.0
                let py = (uv.y - 0.5) * 6.0

                let bass = (self.b0 + self.b1 + self.b2 + self.b3) / 4.0
                let mid = (self.b6 + self.b7 + self.b8 + self.b9) / 4.0
                let treble = (self.b12 + self.b13 + self.b14 + self.b15) / 4.0

                // Frost turbulence — bass pulses intensity
                let intensity = 1.0 + bass * 1.5
                let f1 = sin(px * 2.3 + t * 0.3) * cos(py * 1.7 - t * 0.2) * intensity
                let f2 = sin((px + py) * 1.5 + t * 0.5) * 0.5
                let f3 = cos(px * 3.1 - py * 2.7 + t * 0.4) * 0.3
                let frost = (f1 + f2 + f3) * 0.5 + 0.5

                // Crystal facets — mid drives refraction
                let gx = sin(px * 3.14159 + f1 * 2.0 + mid * 3.0) * 0.5 + 0.5
                let gy = sin(py * 3.14159 + f2 * 2.0 + mid * 3.0) * 0.5 + 0.5
                let facet = gx * gy

                // Ice color: deep blue -> cyan -> white
                let ice_r = 0.3 * frost + 0.3 * facet + treble * 0.2
                let ice_g = 0.5 * frost + 0.5 * facet + mid * 0.15
                let ice_b = 0.8 * frost + 0.7 * facet + bass * 0.1

                // Treble sparkles
                let sparkle_seed = sin(px * 7.0 + t) * cos(py * 9.0 - t * 0.7)
                let sparkle = smoothstep(0.75 - treble * 0.3, 1.0, sparkle_seed) * 0.8

                let r = clamp(ice_r + sparkle, 0.0, 1.0)
                let g = clamp(ice_g + sparkle, 0.0, 1.0)
                let b = clamp(ice_b + sparkle * 0.7, 0.0, 1.0)

                // Vignette — amplitude brightens
                let vp = vec2((uv.x - 0.5) * aspect, uv.y - 0.5)
                let v = clamp(1.0 - length(vp) * (0.8 - self.amplitude * 0.3), 0.3, 1.0)

                return vec4(r * v, g * v, b * v, 1.0)
            }
        }
    }

    // ========================================================================
    // Style G: Lava / Magma
    // ========================================================================
    let SpectrumLava = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time

                let bass = (self.b0 + self.b1 + self.b2 + self.b3) / 4.0
                let mid = (self.b6 + self.b7 + self.b8 + self.b9) / 4.0
                let treble = (self.b12 + self.b13 + self.b14 + self.b15) / 4.0

                // Lava flow turbulence
                let mut px = (uv.x - 0.5) * 4.0
                let mut py = (uv.y - 0.5) * 4.0 - t * 0.3

                let c_a = 0.6
                let s_a = 0.8
                let mut freq = 1.5

                // Octave 1
                let phase1 = freq * (px * s_a + py * c_a) + t * 1.0
                px = px + 0.6 * c_a * sin(phase1) / freq
                py = py + 0.6 * s_a * sin(phase1) / freq
                let rx1 = px * c_a - py * s_a
                let ry1 = px * s_a + py * c_a
                px = rx1
                py = ry1
                freq = freq * 1.5
                // Octave 2
                let phase2 = freq * (px * s_a + py * c_a) + t * 1.5
                px = px + 0.6 * c_a * sin(phase2) / freq
                py = py + 0.6 * s_a * sin(phase2) / freq
                let rx2 = px * c_a - py * s_a
                let ry2 = px * s_a + py * c_a
                px = rx2
                py = ry2
                freq = freq * 1.5
                // Octave 3
                let phase3 = freq * (px * s_a + py * c_a) + t * 2.0
                px = px + 0.6 * c_a * sin(phase3) / freq
                freq = freq * 1.5
                // Octave 4
                let phase4 = freq * (px * s_a + py * c_a) + t * 2.5

                let turb = sin(phase1) * 0.4 + sin(phase2) * 0.3 + sin(phase3) * 0.2 + sin(phase4) * 0.1
                let lava = clamp(turb * 0.5 + 0.5 + bass * 0.3, 0.0, 1.0)

                // Lava color ramp: dark rock → deep red → orange → bright yellow
                let lava_r = smoothstep(0.2, 0.6, lava) * (1.0 + bass * 0.5)
                let lava_g = smoothstep(0.5, 0.9, lava) * 0.6 * (1.0 + mid * 0.4)
                let lava_b = smoothstep(0.8, 1.0, lava) * 0.15 + treble * 0.1

                // Cracks glow — bass makes them pulse
                let crack = smoothstep(0.55, 0.65, lava) * (1.0 - smoothstep(0.65, 0.75, lava))
                let crack_glow = crack * (1.5 + bass * 2.0)

                let r = clamp(lava_r + crack_glow * 1.0, 0.0, 1.0)
                let g = clamp(lava_g + crack_glow * 0.4, 0.0, 1.0)
                let b = clamp(lava_b + crack_glow * 0.05, 0.0, 1.0)

                // Dark rock background
                let rock = (1.0 - lava) * 0.08
                let final_r = max(r, rock)
                let final_g = max(g, rock * 0.5)
                let final_b = max(b, rock * 0.3)

                // Tonemapping
                let tm = vec3(final_r * 1.3, final_g * 1.3, final_b * 1.3)
                return vec4(
                    tm.x / (1.0 + abs(tm.x)),
                    tm.y / (1.0 + abs(tm.y)),
                    tm.z / (1.0 + abs(tm.z)),
                    1.0
                )
            }
        }
    }

    // ========================================================================
    // Style H: Desert
    // ========================================================================
    let SpectrumDesert = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time

                let bass = (self.b0 + self.b1 + self.b2 + self.b3) / 4.0
                let mid = (self.b6 + self.b7 + self.b8 + self.b9) / 4.0
                let treble = (self.b12 + self.b13 + self.b14 + self.b15) / 4.0

                // Sky gradient (top)
                let horizon = 0.45 + sin(uv.x * 2.0 + t * 0.1) * 0.01
                let is_sky = 1.0 - step(horizon, uv.y)

                let sky_r = mix(0.95, 0.4, uv.y / horizon) * is_sky
                let sky_g = mix(0.7, 0.5, uv.y / horizon) * is_sky
                let sky_b = mix(0.4, 0.8, uv.y / horizon) * is_sky

                // Sand dunes
                let dune_y = horizon + sin(uv.x * 5.0 + t * 0.15) * 0.03 + sin(uv.x * 12.0) * 0.015
                let sand_depth = (uv.y - dune_y) / (1.0 - dune_y)
                let is_sand = step(horizon, uv.y)

                // Sand color with wind ripples — audio drives wind
                let wind = sin(uv.x * 30.0 + t * 2.0 + bass * 5.0) * 0.02 * mid
                let ripple = sin(uv.x * 60.0 - t * 1.5 + wind * 10.0) * 0.5 + 0.5
                let sand_r = (0.85 - sand_depth * 0.15 + ripple * 0.05) * is_sand
                let sand_g = (0.65 - sand_depth * 0.15 + ripple * 0.03) * is_sand
                let sand_b = (0.35 - sand_depth * 0.1) * is_sand

                // Heat shimmer — bass drives distortion
                let shimmer = sin(uv.x * 40.0 + t * 3.0) * sin(uv.y * 100.0 + t * 5.0) * bass * 0.05
                let shimmer_alpha = exp(-abs(uv.y - horizon) * 30.0) * bass

                // Sun — amplitude controls glow
                let sun_pos = vec2(0.75, 0.2)
                let sun_dist = length(vec2(uv.x - sun_pos.x, uv.y - sun_pos.y))
                let sun = exp(-sun_dist * (6.0 - self.amplitude * 3.0)) * (0.5 + self.amplitude * 0.5)
                let sun_core = exp(-sun_dist * 30.0) * 1.5

                let r = sky_r + sand_r + sun * 1.0 + sun_core + shimmer * shimmer_alpha
                let g = sky_g + sand_g + sun * 0.7 + sun_core * 0.9 + shimmer * shimmer_alpha * 0.5
                let b = sky_b + sand_b + sun * 0.2 + sun_core * 0.6

                return vec4(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0)
            }
        }
    }

    // ========================================================================
    // Style I: Blue Sky
    // ========================================================================
    let SpectrumSky = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time

                let bass = (self.b0 + self.b1 + self.b2 + self.b3) / 4.0
                let mid = (self.b6 + self.b7 + self.b8 + self.b9) / 4.0
                let treble = (self.b12 + self.b13 + self.b14 + self.b15) / 4.0

                // Sky gradient
                let sky_r = 0.3 - uv.y * 0.15
                let sky_g = 0.5 - uv.y * 0.1
                let sky_b = 0.85 - uv.y * 0.2

                // Clouds — layered noise, bass pushes them
                let cx1 = uv.x * 4.0 + t * 0.15 + bass * 0.5
                let cy1 = uv.y * 3.0
                let cloud1 = sin(cx1) * cos(cy1 * 1.3 + sin(cx1 * 0.7) * 0.5) * 0.5 + 0.5
                let cloud1_shape = smoothstep(0.35, 0.7, cloud1) * smoothstep(0.7, 0.2, uv.y)

                let cx2 = uv.x * 7.0 + t * 0.25 + mid * 0.3
                let cy2 = uv.y * 5.0 + 1.0
                let cloud2 = sin(cx2 + 2.0) * cos(cy2 * 1.1 + sin(cx2 * 0.5) * 0.3) * 0.5 + 0.5
                let cloud2_shape = smoothstep(0.4, 0.75, cloud2) * smoothstep(0.6, 0.15, uv.y) * 0.7

                let cloud = max(cloud1_shape, cloud2_shape)

                // Sun — amplitude controls brightness
                let sun_pos = vec2(0.3, 0.15)
                let sun_dist = length(vec2(uv.x - sun_pos.x, uv.y - sun_pos.y))
                let sun = exp(-sun_dist * (8.0 - self.amplitude * 4.0)) * (0.3 + self.amplitude * 0.4)
                let sun_core = exp(-sun_dist * 40.0) * 2.0

                // God rays from treble
                let ray_angle = (uv.x - sun_pos.x) * 8.0
                let ray = max(sin(ray_angle + t * 0.5), 0.0) * exp(-sun_dist * 3.0) * treble * 0.3

                let r = sky_r * (1.0 - cloud) + cloud * 0.95 + sun * 0.9 + sun_core + ray * 0.8
                let g = sky_g * (1.0 - cloud) + cloud * 0.95 + sun * 0.8 + sun_core + ray * 0.6
                let b = sky_b * (1.0 - cloud) + cloud * 1.0 + sun * 0.3 + sun_core * 0.7 + ray * 0.3

                return vec4(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0)
            }
        }
    }

    // ========================================================================
    // Style J: Cloud Sea (云海)
    // ========================================================================
    let SpectrumCloudSea = set_type_default() do VisualizerBase{
        width: Fill
        height: Fill
        draw_bg +: {
            pixel: fn() {
                let uv = self.pos
                let t = self.time
                let aspect = self.rect_size.x / max(self.rect_size.y, 0.001)

                let bass = (self.b0 + self.b1 + self.b2 + self.b3) / 4.0
                let mid = (self.b6 + self.b7 + self.b8 + self.b9) / 4.0
                let treble = (self.b12 + self.b13 + self.b14 + self.b15) / 4.0

                // Perspective-ish Y coordinate — clouds recede into distance
                let py = max(uv.y - 0.3, 0.001)
                let depth = 1.0 / py
                let px = (uv.x - 0.5) * aspect * depth * 0.5

                // Cloud layers with turbulence — bass drives flow
                let flow = t * 0.2 + bass * 0.8
                let c1 = sin(px * 1.5 + flow) * cos(depth * 0.8 + flow * 0.7) * 0.5 + 0.5
                let c2 = sin(px * 3.0 + flow * 1.3 + 1.0) * cos(depth * 1.5 - flow * 0.5) * 0.5 + 0.5
                let c3 = sin(px * 5.0 - flow * 0.8 + 2.0) * cos(depth * 2.5 + flow * 1.1) * 0.5 + 0.5
                let cloud_density = c1 * 0.5 + c2 * 0.3 + c3 * 0.2

                // Cloud shape — thicker at horizon, thinner overhead
                let horizon_fade = smoothstep(0.3, 0.5, uv.y)
                let cloud = smoothstep(0.3 - mid * 0.15, 0.7, cloud_density) * horizon_fade

                // Sky above clouds — golden sunrise gradient
                let sky_r = mix(0.15, 0.9, smoothstep(0.0, 0.4, uv.y)) * (1.0 - horizon_fade * 0.3)
                let sky_g = mix(0.1, 0.5, smoothstep(0.0, 0.5, uv.y)) * (1.0 - horizon_fade * 0.2)
                let sky_b = mix(0.3, 0.3, smoothstep(0.0, 0.3, uv.y))

                // Cloud color — golden-lit tops, purple-blue shadows
                let lit_r = 0.95 + treble * 0.1
                let lit_g = 0.85 + mid * 0.05
                let lit_b = 0.75
                let shadow_r = 0.3 + bass * 0.15
                let shadow_g = 0.25
                let shadow_b = 0.45 + treble * 0.1

                let cloud_light = smoothstep(0.4, 0.8, cloud_density)
                let cloud_r = mix(shadow_r, lit_r, cloud_light) * cloud
                let cloud_g = mix(shadow_g, lit_g, cloud_light) * cloud
                let cloud_b = mix(shadow_b, lit_b, cloud_light) * cloud

                // Horizon glow — amplitude drives intensity
                let horizon_glow = exp(-abs(uv.y - 0.35) * (8.0 - self.amplitude * 4.0)) * (0.3 + self.amplitude * 0.4)

                let r = sky_r * (1.0 - cloud) + cloud_r + horizon_glow * 0.8
                let g = sky_g * (1.0 - cloud) + cloud_g + horizon_glow * 0.5
                let b = sky_b * (1.0 - cloud) + cloud_b + horizon_glow * 0.2

                return vec4(clamp(r, 0.0, 1.0), clamp(g, 0.0, 1.0), clamp(b, 0.0, 1.0), 1.0)
            }
        }
    }

    // ========================================================================
    // UI Layout
    // ========================================================================
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(900, 700)
                pass.clear_color: #x0a0a12
                body +: {
                    View{width: Fill height: Fill flow: Down

                        // Shader visualization area — stack with Overlay
                        View{width: Fill height: Fill flow: Overlay
                            vis_bars := SpectrumBars{}
                            vis_circle := SpectrumCircular{ visible: false }
                            vis_wave := SpectrumWave{ visible: false }
                            vis_fire := SpectrumFire{ visible: false }
                            vis_stars := SpectrumStars{ visible: false }
                            vis_ice := SpectrumIce{ visible: false }
                            vis_lava := SpectrumLava{ visible: false }
                            vis_desert := SpectrumDesert{ visible: false }
                            vis_sky := SpectrumSky{ visible: false }
                            vis_cloud := SpectrumCloudSea{ visible: false }
                            // Dynamic shader container (Splash runtime eval)
                            vis_dynamic := Splash{ width: Fill height: Fill visible: false }
                        }

                        // Bottom control bar
                        SolidView{width: Fill height: Fit draw_bg.color: #x111118 padding: Inset{left: 20. right: 20. top: 14. bottom: 14.} flow: Down spacing: 10 new_batch: true

                            // Song info
                            View{width: Fill height: Fit flow: Right spacing: 12 align: Align{y: 0.5}
                                song_label := Label{text: "Click a song to start" draw_text.color: #xccccdd draw_text.text_style.font_size: 13}
                                Filler{}
                                time_label := Label{text: "" draw_text.color: #x888899 draw_text.text_style.font_size: 11}
                            }

                            // Playback controls + viz mode
                            View{width: Fill height: Fit flow: Right spacing: 12 align: Align{y: 0.5}
                                play_btn := Button{text: "Play" width: 80 height: 36}
                                stop_btn := Button{text: "Stop" width: 80 height: 36}
                                Filler{}
                                bars_btn := Button{text: "Bars" width: Fit height: 32}
                                circle_btn := Button{text: "Circle" width: Fit height: 32}
                                wave_btn := Button{text: "Wave" width: Fit height: 32}
                                fire_btn := Button{text: "Fire" width: Fit height: 32}
                                stars_btn := Button{text: "Stars" width: Fit height: 32}
                                ice_btn := Button{text: "Ice" width: Fit height: 32}
                                lava_btn := Button{text: "Lava" width: Fit height: 32}
                                desert_btn := Button{text: "Desert" width: Fit height: 32}
                                sky_btn := Button{text: "Sky" width: Fit height: 32}
                                cloud_btn := Button{text: "CloudSea" width: Fit height: 32}
                            }

                            // Preset songs
                            View{width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.5}
                                demo1 := Button{text: "Ambient Flow" width: Fit height: 30}
                                demo2 := Button{text: "Electronic Pulse" width: Fit height: 30}
                                demo3 := Button{text: "Synth Dream" width: Fit height: 30}
                            }

                            // Shader effect prompt input
                            View{width: Fill height: Fit flow: Right spacing: 10 align: Align{y: 0.5}
                                Label{text: "Effect:" draw_text.color: #x888899 draw_text.text_style.font_size: 11 width: Fit}
                                effect_input := TextInput{
                                    width: Fill height: 32
                                    empty_text: "Describe effect: fire, stars, wave, ocean..."
                                    draw_bg.color: #x1a1a2e
                                    draw_text.color: #xccccdd
                                    draw_text.text_style.font_size: 11
                                }
                                apply_effect_btn := Button{text: "Apply" width: Fit height: 32}
                            }
                        }
                    }
                }
            }
        }
    }
}

// ============================================================================
// DrawVisualizer — instance vars for shader
// ============================================================================

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
    #[live]
    b0: f32,
    #[live]
    b1: f32,
    #[live]
    b2: f32,
    #[live]
    b3: f32,
    #[live]
    b4: f32,
    #[live]
    b5: f32,
    #[live]
    b6: f32,
    #[live]
    b7: f32,
    #[live]
    b8: f32,
    #[live]
    b9: f32,
    #[live]
    b10: f32,
    #[live]
    b11: f32,
    #[live]
    b12: f32,
    #[live]
    b13: f32,
    #[live]
    b14: f32,
    #[live]
    b15: f32,
}

// ============================================================================
// Visualizer Widget
// ============================================================================

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
    area: Area,
    #[live(true)]
    visible: bool,
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

                // Read spectrum from shared state
                let state = get_audio_state();
                self.draw_bg.amplitude = state.amplitude.get() as f32;

                if let Ok(bands) = state.spectrum.lock() {
                    self.draw_bg.b0 = bands[0];
                    self.draw_bg.b1 = bands[1];
                    self.draw_bg.b2 = bands[2];
                    self.draw_bg.b3 = bands[3];
                    self.draw_bg.b4 = bands[4];
                    self.draw_bg.b5 = bands[5];
                    self.draw_bg.b6 = bands[6];
                    self.draw_bg.b7 = bands[7];
                    self.draw_bg.b8 = bands[8];
                    self.draw_bg.b9 = bands[9];
                    self.draw_bg.b10 = bands[10];
                    self.draw_bg.b11 = bands[11];
                    self.draw_bg.b12 = bands[12];
                    self.draw_bg.b13 = bands[13];
                    self.draw_bg.b14 = bands[14];
                    self.draw_bg.b15 = bands[15];
                }

                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        if !self.visible {
            return DrawStep::done();
        }
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.draw_bg.draw_abs(cx, rect);
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

// ============================================================================
// App
// ============================================================================

const SHADER_PROMPT_PATH: &str = "/tmp/shader_music_player_prompt.txt";
const SHADER_RESPONSE_PATH: &str = "/tmp/shader_music_player_response.splash";

static PENDING_SHADER: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static SHADER_CACHE: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> =
    std::sync::Mutex::new(None);

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    audio_signal: SignalToUI,
    #[rust]
    shader_signal: SignalToUI,
    #[rust]
    audio_initialized: bool,
    #[rust]
    vis_mode: u8,
    #[rust]
    waiting_for_shader: bool,
}

impl MatchEvent for App {
    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let state = get_audio_state();

        if self.ui.button(cx, ids!(play_btn)).clicked(actions) {
            log!("[APP] Play/Pause clicked");
            state.toggle();
        }
        if self.ui.button(cx, ids!(stop_btn)).clicked(actions) {
            log!("[APP] Stop clicked");
            state.stop();
        }
        if self.ui.button(cx, ids!(demo1)).clicked(actions) {
            log!("[APP] Demo1 clicked");
            self.load_song(cx, 0);
        }
        if self.ui.button(cx, ids!(demo2)).clicked(actions) {
            log!("[APP] Demo2 clicked");
            self.load_song(cx, 1);
        }
        if self.ui.button(cx, ids!(demo3)).clicked(actions) {
            log!("[APP] Demo3 clicked");
            self.load_song(cx, 2);
        }
        if self.ui.button(cx, ids!(bars_btn)).clicked(actions) {
            self.set_vis_mode(cx, 0);
        }
        if self.ui.button(cx, ids!(circle_btn)).clicked(actions) {
            self.set_vis_mode(cx, 1);
        }
        if self.ui.button(cx, ids!(wave_btn)).clicked(actions) {
            self.set_vis_mode(cx, 2);
        }
        if self.ui.button(cx, ids!(fire_btn)).clicked(actions) {
            self.set_vis_mode(cx, 3);
        }
        if self.ui.button(cx, ids!(stars_btn)).clicked(actions) {
            self.set_vis_mode(cx, 4);
        }
        if self.ui.button(cx, ids!(ice_btn)).clicked(actions) {
            self.set_vis_mode(cx, 5);
        }
        if self.ui.button(cx, ids!(lava_btn)).clicked(actions) {
            self.set_vis_mode(cx, 6);
        }
        if self.ui.button(cx, ids!(desert_btn)).clicked(actions) {
            self.set_vis_mode(cx, 7);
        }
        if self.ui.button(cx, ids!(sky_btn)).clicked(actions) {
            self.set_vis_mode(cx, 8);
        }
        if self.ui.button(cx, ids!(cloud_btn)).clicked(actions) {
            self.set_vis_mode(cx, 9);
        }
        // Apply effect: from button click or Enter key in TextInput (only one trigger)
        let apply_prompt =
            if let Some((text, _)) = self.ui.text_input(cx, ids!(effect_input)).returned(actions) {
                log!("[APP] Enter pressed in effect input");
                Some(text)
            } else if self.ui.button(cx, ids!(apply_effect_btn)).clicked(actions) {
                log!("[APP] Apply button clicked");
                Some(self.ui.text_input(cx, ids!(effect_input)).text())
            } else {
                None
            };
        if let Some(prompt) = apply_prompt {
            log!("[APP] Applying effect prompt: '{}'", prompt);
            self.apply_effect(cx, &prompt);
        }
    }

    fn handle_startup(&mut self, cx: &mut Cx) {
        log!("[APP] handle_startup called");
        if !self.audio_initialized {
            self.audio_initialized = true;
            let state = get_audio_state();
            audio::start_audio_output(cx, state, self.audio_signal.clone());
            log!("[APP] Audio output registered");
        }
    }

    fn handle_audio_devices(&mut self, cx: &mut Cx, devices: &AudioDevicesEvent) {
        log!(
            "[APP] handle_audio_devices: {} devices",
            devices.descs.len()
        );
        let default_output = devices.default_output();
        if !default_output.is_empty() {
            cx.use_audio_outputs(&default_output);
            log!("[APP] Using default audio output");
        }
    }

    fn handle_signal(&mut self, cx: &mut Cx) {
        if self.audio_signal.check_and_clear() {
            log!("[APP] Audio signal received");
            let state = get_audio_state();
            let position = state.position_secs.get();
            let duration = state.duration_secs.get();
            let is_playing = state.is_playing.load(Ordering::Relaxed);

            if duration > 0.0 {
                let pos_min = (position / 60.0) as u32;
                let pos_sec = (position % 60.0) as u32;
                let dur_min = (duration / 60.0) as u32;
                let dur_sec = (duration % 60.0) as u32;
                let time_text = format!("{}:{:02} / {}:{:02}", pos_min, pos_sec, dur_min, dur_sec);
                self.ui.label(cx, ids!(time_label)).set_text(cx, &time_text);
            }

            let play_text = if is_playing { "Pause" } else { "Play" };
            self.ui.button(cx, ids!(play_btn)).set_text(cx, play_text);

            self.ui.redraw(cx);
        }

        // Check for AI-generated shader response
        if self.shader_signal.check_and_clear() && self.waiting_for_shader {
            self.waiting_for_shader = false;
            let shader_code = if let Ok(mut guard) = PENDING_SHADER.lock() {
                guard.take()
            } else {
                None
            };
            if let Some(shader_code) = shader_code {
                log!("[APP] AI shader received, {} bytes", shader_code.len());

                // Hide all predefined visualizers
                let vis_ids: &[&[LiveId]] = &[
                    ids!(vis_bars),
                    ids!(vis_circle),
                    ids!(vis_wave),
                    ids!(vis_fire),
                    ids!(vis_stars),
                    ids!(vis_ice),
                    ids!(vis_lava),
                    ids!(vis_desert),
                    ids!(vis_sky),
                    ids!(vis_cloud),
                ];
                for vid in vis_ids.iter() {
                    if let Some(mut v) = self.ui.widget(cx, vid).borrow_mut::<Visualizer>() {
                        v.visible = false;
                    }
                }

                // Show Splash container and set shader
                let splash_widget = self.ui.widget(cx, ids!(vis_dynamic));
                if let Some(mut splash) = splash_widget.borrow_mut::<Splash>() {
                    splash.view.set_visible(cx, true);
                    splash.set_text(cx, &shader_code);
                    log!("[APP] Dynamic shader rendered via set_text");
                } else {
                    log!("[APP] ERROR: Could not borrow Splash widget!");
                }
                self.ui
                    .label(cx, ids!(song_label))
                    .set_text(cx, "AI Shader Active");
                self.ui.redraw(cx);
            } else {
                log!("[APP] Shader signal received but no pending shader code");
                self.ui
                    .label(cx, ids!(song_label))
                    .set_text(cx, "Shader generation failed");
                self.ui.redraw(cx);
            }
        }
    }
}

impl App {
    fn load_song(&mut self, cx: &mut Cx, index: usize) {
        if index >= SONGS.len() {
            return;
        }
        let (name, url) = SONGS[index];
        let state = get_audio_state();
        state.stop();

        self.ui
            .label(cx, ids!(song_label))
            .set_text(cx, &format!("Loading: {}...", name));
        self.ui.redraw(cx);

        audio::download_and_decode(url.to_string(), state, self.audio_signal.clone());
    }

    fn apply_effect(&mut self, cx: &mut Cx, prompt: &str) {
        let prompt_lower = prompt.to_lowercase();
        // Try predefined effects first
        if let Some(m) = match_effect_prompt(&prompt_lower) {
            self.set_vis_mode(cx, m);
            let names = [
                "Bars", "Circle", "Wave", "Fire", "Stars", "Ice", "Lava", "Desert", "Sky",
                "CloudSea",
            ];
            self.ui
                .label(cx, ids!(song_label))
                .set_text(cx, &format!("Effect: {}", names[m as usize]));
            self.ui.redraw(cx);
            return;
        }

        // Check shader cache
        {
            let cache_guard = SHADER_CACHE.lock().unwrap();
            if let Some(cache) = cache_guard.as_ref() {
                if let Some(cached_code) = cache.get(&prompt_lower) {
                    log!("[APP] Shader cache hit for: '{}'", prompt);
                    if let Ok(mut guard) = PENDING_SHADER.lock() {
                        *guard = Some(cached_code.clone());
                    }
                    self.waiting_for_shader = true;
                    self.shader_signal.set();
                    self.ui
                        .label(cx, ids!(song_label))
                        .set_text(cx, &format!("Cached: {}", prompt));
                    self.ui.redraw(cx);
                    return;
                }
            }
        }

        // Don't start a new request if one is already pending
        if self.waiting_for_shader {
            log!("[APP] Already waiting for shader, ignoring duplicate request");
            return;
        }
        // Write prompt to file for AI to generate shader
        log!("[APP] Requesting AI shader for: '{}'", prompt);
        let prompt_for_cache = prompt_lower.clone();
        // Remove old response file
        let _ = std::fs::remove_file(SHADER_RESPONSE_PATH);
        // Write prompt
        if let Err(e) = std::fs::write(SHADER_PROMPT_PATH, prompt) {
            log!("[APP] ERROR writing prompt file: {}", e);
            return;
        }
        self.waiting_for_shader = true;
        self.ui
            .label(cx, ids!(song_label))
            .set_text(cx, &format!("AI generating: {}...", prompt));
        self.ui.redraw(cx);

        // Spawn a thread to poll for response file and read it
        let signal = self.shader_signal.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            loop {
                if std::path::Path::new(SHADER_RESPONSE_PATH).exists() {
                    // Small delay to ensure file is fully written
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    // Read file in this thread (no race with main thread)
                    match std::fs::read_to_string(SHADER_RESPONSE_PATH) {
                        Ok(code) => {
                            let _ = std::fs::remove_file(SHADER_RESPONSE_PATH);
                            let _ = std::fs::remove_file(SHADER_PROMPT_PATH);
                            log!("[APP] Shader file read: {} bytes", code.len());
                            // Store in cache
                            {
                                let mut cache_guard = SHADER_CACHE.lock().unwrap();
                                let cache =
                                    cache_guard.get_or_insert_with(std::collections::HashMap::new);
                                cache.insert(prompt_for_cache, code.clone());
                                log!("[APP] Shader cached for prompt");
                            }
                            if let Ok(mut guard) = PENDING_SHADER.lock() {
                                *guard = Some(code);
                            }
                            signal.set();
                        }
                        Err(e) => {
                            log!("[APP] ERROR reading shader response: {}", e);
                        }
                    }
                    return;
                }
                if start.elapsed() > std::time::Duration::from_secs(120) {
                    log!("[APP] Shader response timeout (120s)");
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        });
    }

    fn set_vis_mode(&mut self, cx: &mut Cx, mode: u8) {
        log!("[APP] set_vis_mode({})", mode);
        self.vis_mode = mode;
        let vis_ids: &[&[LiveId]] = &[
            ids!(vis_bars),
            ids!(vis_circle),
            ids!(vis_wave),
            ids!(vis_fire),
            ids!(vis_stars),
            ids!(vis_ice),
            ids!(vis_lava),
            ids!(vis_desert),
            ids!(vis_sky),
            ids!(vis_cloud),
        ];
        for (i, vid) in vis_ids.iter().enumerate() {
            if let Some(mut v) = self.ui.widget(cx, vid).borrow_mut::<Visualizer>() {
                v.visible = i as u8 == mode;
            }
        }
        // Hide dynamic Splash when switching to predefined
        if let Some(mut splash) = self.ui.widget(cx, ids!(vis_dynamic)).borrow_mut::<Splash>() {
            splash.view.set_visible(cx, false);
        }
        self.ui.redraw(cx);
    }
}

fn _removed_generate_dynamic_shader(prompt: &str) -> Option<String> {
    let pixel_fn = if prompt.contains("ice")
        || prompt.contains("冰")
        || prompt.contains("frost")
        || prompt.contains("crystal")
        || prompt.contains("冻")
    {
        // Ice / Crystal shader
        r#"pixel: fn() {
                let uv = self.pos
                let t = self.draw_pass.time
                let aspect = self.rect_size.x / max(self.rect_size.y, 0.001)
                let px = (uv.x - 0.5) * aspect * 6.0
                let py = (uv.y - 0.5) * 6.0

                // Frost turbulence
                let f1 = sin(px * 2.3 + t * 0.3) * cos(py * 1.7 - t * 0.2)
                let f2 = sin((px + py) * 1.5 + t * 0.5) * 0.5
                let f3 = cos(px * 3.1 - py * 2.7 + t * 0.4) * 0.3
                let frost = (f1 + f2 + f3) * 0.5 + 0.5

                // Crystal facets
                let gx = sin(px * 3.14159 + f1 * 2.0) * 0.5 + 0.5
                let gy = sin(py * 3.14159 + f2 * 2.0) * 0.5 + 0.5
                let facet = gx * gy

                // Ice color: deep blue -> cyan -> white
                let ice_r = 0.3 * frost + 0.3 * facet
                let ice_g = 0.5 * frost + 0.5 * facet
                let ice_b = 0.8 * frost + 0.7 * facet

                // Sparkle
                let sparkle_seed = sin(px * 7.0 + t) * cos(py * 9.0 - t * 0.7)
                let sparkle = smoothstep(0.8, 1.0, sparkle_seed) * 0.8

                let r = clamp(ice_r + sparkle, 0.0, 1.0)
                let g = clamp(ice_g + sparkle, 0.0, 1.0)
                let b = clamp(ice_b + sparkle * 0.7, 0.0, 1.0)

                // Vignette
                let p = vec2((uv.x - 0.5) * aspect, uv.y - 0.5)
                let v = clamp(1.0 - length(p) * 0.8, 0.3, 1.0)

                return Pal.premul(vec4(r * v, g * v, b * v, 1.0))
            }"#
    } else if prompt.contains("aurora")
        || prompt.contains("极光")
        || prompt.contains("northern light")
    {
        // Aurora Borealis shader
        r#"pixel: fn() {
                let uv = self.pos
                let t = self.draw_pass.time

                let w1 = sin(uv.x * 4.0 + t * 0.7) * 0.15
                let w2 = sin(uv.x * 6.0 - t * 0.5 + 1.0) * 0.1
                let w3 = sin(uv.x * 10.0 + t * 1.2) * 0.05

                let wave_y = 0.4 + w1 + w2 + w3
                let dist = abs(uv.y - wave_y)
                let width = 0.15
                let glow = exp(-dist * dist / (width * width * 2.0))

                let hue_shift = uv.x * 0.3 + t * 0.1
                let r = glow * (0.1 + 0.5 * sin(hue_shift * 3.0 + 2.0) * 0.5)
                let g = glow * 0.8
                let b = glow * (0.3 + 0.5 * sin(hue_shift * 2.0 + 4.0) * 0.5)

                let wave_y2 = 0.6 + sin(uv.x * 3.0 - t * 0.4) * 0.1
                let dist2 = abs(uv.y - wave_y2)
                let glow2 = exp(-dist2 * dist2 / (0.1 * 0.1)) * 0.5
                let r2 = glow2 * 0.6
                let g2 = glow2 * 0.2
                let b2 = glow2 * 0.8

                let sky = 0.02 + uv.y * 0.03

                return Pal.premul(vec4(
                    clamp(r + r2 + sky * 0.3, 0.0, 1.0),
                    clamp(g + g2 + sky * 0.3, 0.0, 1.0),
                    clamp(b + b2 + sky * 0.8, 0.0, 1.0),
                    1.0
                ))
            }"#
    } else if prompt.contains("ocean")
        || prompt.contains("海")
        || prompt.contains("water")
        || prompt.contains("水")
    {
        // Ocean waves shader
        r#"pixel: fn() {
                let uv = self.pos
                let t = self.draw_pass.time

                let w1 = sin(uv.x * 8.0 + t * 2.0) * 0.03
                let w2 = sin(uv.x * 12.0 - t * 1.5) * 0.02
                let w3 = sin(uv.x * 20.0 + t * 3.0) * 0.01
                let wave = w1 + w2 + w3
                let surface = 0.5 + wave
                let depth = uv.y - surface

                let sky_r = 0.1
                let sky_g = 0.15
                let sky_b = 0.3 + (1.0 - uv.y) * 0.2

                let ocean_r = 0.05 * exp(-depth * 3.0)
                let ocean_g = 0.1 + 0.3 * exp(-depth * 2.0)
                let ocean_b = 0.3 + 0.5 * exp(-depth * 1.5)

                let foam = exp(-abs(depth) * 40.0) * 0.6
                let is_water = step(0.0, depth)

                let r = sky_r * (1.0 - is_water) + ocean_r * is_water + foam
                let g = sky_g * (1.0 - is_water) + ocean_g * is_water + foam
                let b = sky_b * (1.0 - is_water) + ocean_b * is_water + foam * 0.8

                let cx1 = sin(uv.x * 15.0 + t * 1.5) * sin(uv.y * 15.0 + t * 1.2)
                let caustic = max(cx1, 0.0) * is_water * exp(-depth * 4.0) * 0.3

                return Pal.premul(vec4(
                    clamp(r + caustic * 0.3, 0.0, 1.0),
                    clamp(g + caustic * 0.5, 0.0, 1.0),
                    clamp(b + caustic * 0.2, 0.0, 1.0),
                    1.0
                ))
            }"#
    } else if prompt.contains("neon")
        || prompt.contains("霓虹")
        || prompt.contains("cyber")
        || prompt.contains("赛博")
    {
        // Neon / Cyberpunk shader
        r#"pixel: fn() {
                let uv = self.pos
                let t = self.draw_pass.time

                let grid_x = abs(sin(uv.x * 20.0 * 3.14159))
                let grid_y = abs(sin(uv.y * 20.0 * 3.14159))
                let grid = min(grid_x, grid_y)
                let line = smoothstep(0.0, 0.05, 1.0 - grid)

                let strip1 = exp(-abs(uv.y - 0.3 - sin(uv.x * 5.0 + t) * 0.1) * 20.0)
                let strip2 = exp(-abs(uv.y - 0.7 + sin(uv.x * 4.0 - t * 0.8) * 0.1) * 20.0)

                let r = strip1 * 1.0 + line * 0.1
                let g = strip2 * 0.3 + line * 0.3
                let b = strip2 * 1.0 + strip1 * 0.3 + line * 0.15

                let scan = 0.95 + 0.05 * sin(uv.y * 200.0 + t * 5.0)

                return Pal.premul(vec4(
                    clamp(r * scan, 0.0, 1.0),
                    clamp(g * scan, 0.0, 1.0),
                    clamp(b * scan, 0.0, 1.0),
                    1.0
                ))
            }"#
    } else if prompt.contains("grass")
        || prompt.contains("prairie")
        || prompt.contains("草")
        || prompt.contains("原")
        || prompt.contains("meadow")
        || prompt.contains("field")
    {
        // Grassland / Prairie shader
        r#"pixel: fn() {
                let uv = self.pos
                let t = self.draw_pass.time

                // Sky gradient
                let sky_b = smoothstep(0.3, 0.0, uv.y) * 0.6 + 0.2
                let sky_g = smoothstep(0.4, 0.0, uv.y) * 0.3 + 0.1
                let sky_r = smoothstep(0.3, 0.0, uv.y) * 0.15 + 0.05

                // Ground line
                let ground = 0.55 + sin(uv.x * 3.0 + t * 0.2) * 0.02 + sin(uv.x * 7.0) * 0.01
                let is_ground = step(ground, uv.y)

                // Grass color with wind
                let wind = sin(uv.x * 15.0 + t * 2.0) * 0.02 + sin(uv.x * 25.0 - t * 1.5) * 0.01
                let grass_height = (uv.y - ground) / (1.0 - ground)
                let grass_g = 0.35 + 0.25 * (1.0 - grass_height) + wind * 2.0
                let grass_r = 0.15 + 0.1 * grass_height
                let grass_b = 0.05

                // Grass blade tips (lighter)
                let blade = sin(uv.x * 60.0 + t * 0.5 + sin(uv.x * 20.0) * 2.0)
                let blade_bright = smoothstep(0.7, 1.0, blade) * (1.0 - grass_height) * 0.15

                let r = sky_r * (1.0 - is_ground) + (grass_r + blade_bright) * is_ground
                let g = sky_g * (1.0 - is_ground) + (grass_g + blade_bright * 2.0) * is_ground
                let b = sky_b * (1.0 - is_ground) + grass_b * is_ground

                // Sun glow
                let sun_pos = vec2(0.7, 0.15)
                let sun_dist = length(vec2(uv.x - sun_pos.x, uv.y - sun_pos.y))
                let sun = exp(-sun_dist * 8.0) * 0.4

                return Pal.premul(vec4(
                    clamp(r + sun * 1.0, 0.0, 1.0),
                    clamp(g + sun * 0.8, 0.0, 1.0),
                    clamp(b + sun * 0.3, 0.0, 1.0),
                    1.0
                ))
            }"#
    } else {
        // Generic fallback: colorful plasma effect for any unknown prompt
        r#"pixel: fn() {
                let uv = self.pos
                let t = self.draw_pass.time

                let p1 = sin(uv.x * 10.0 + t * 1.5)
                let p2 = sin(uv.y * 8.0 - t * 1.2)
                let p3 = sin((uv.x + uv.y) * 6.0 + t * 0.8)
                let p4 = sin(length(vec2(uv.x - 0.5, uv.y - 0.5)) * 12.0 - t * 2.0)

                let val = (p1 + p2 + p3 + p4) * 0.25

                let r = sin(val * 3.14159 + 0.0) * 0.5 + 0.5
                let g = sin(val * 3.14159 + 2.094) * 0.5 + 0.5
                let b = sin(val * 3.14159 + 4.188) * 0.5 + 0.5

                return Pal.premul(vec4(r * 0.8, g * 0.8, b * 0.8, 1.0))
            }"#
    };

    // Body goes inside SPLASH_PREFIX: "use mod.prelude.widgets.*View{height:Fit, "
    // Override height to Fill, enable show_bg, override pixel shader.
    let code = format!(
        r#"height: Fill show_bg: true
            draw_bg +: {{
                {}
            }}"#,
        pixel_fn
    );
    log!("[SHADER] Generated code:\n{}", code);
    Some(code)
}

/// Match user's effect prompt to a visualization mode
fn match_effect_prompt(prompt: &str) -> Option<u8> {
    // Ice/crystal keywords
    if prompt.contains("ice")
        || prompt.contains("frost")
        || prompt.contains("crystal")
        || prompt.contains("冰")
        || prompt.contains("霜")
        || prompt.contains("冻")
        || prompt.contains("水晶")
    {
        return Some(5);
    }
    // Lava/magma keywords
    if prompt.contains("lava")
        || prompt.contains("magma")
        || prompt.contains("volcano")
        || prompt.contains("岩浆")
        || prompt.contains("熔岩")
        || prompt.contains("火山")
    {
        return Some(6);
    }
    // Desert keywords
    if prompt.contains("desert")
        || prompt.contains("sand")
        || prompt.contains("dune")
        || prompt.contains("沙漠")
        || prompt.contains("沙丘")
        || prompt.contains("荒漠")
    {
        return Some(7);
    }
    // Sky keywords (before cloud sea, more specific)
    if prompt.contains("sky") || prompt.contains("蓝天") || prompt.contains("天空") {
        return Some(8);
    }
    // Cloud sea keywords
    if prompt.contains("cloud") || prompt.contains("云海") || prompt.contains("云") {
        return Some(9);
    }
    // Fire keywords
    if prompt.contains("fire")
        || prompt.contains("flame")
        || prompt.contains("burn")
        || prompt.contains("火")
        || prompt.contains("焰")
        || prompt.contains("燃")
    {
        return Some(3);
    }
    // Stars/space keywords
    if prompt.contains("star")
        || prompt.contains("space")
        || prompt.contains("galaxy")
        || prompt.contains("nebula")
        || prompt.contains("cosmos")
        || prompt.contains("星")
        || prompt.contains("宇宙")
        || prompt.contains("银河")
    {
        return Some(4);
    }
    // Wave/particle keywords
    if prompt.contains("wave")
        || prompt.contains("particle")
        || prompt.contains("波")
        || prompt.contains("粒子")
    {
        return Some(2);
    }
    // Circle/ring keywords
    if prompt.contains("circle")
        || prompt.contains("ring")
        || prompt.contains("radar")
        || prompt.contains("环")
        || prompt.contains("圆")
        || prompt.contains("雷达")
    {
        return Some(1);
    }
    // Bars/spectrum keywords
    if prompt.contains("bar")
        || prompt.contains("spectrum")
        || prompt.contains("equalizer")
        || prompt.contains("柱")
        || prompt.contains("频谱")
        || prompt.contains("均衡")
    {
        return Some(0);
    }
    None
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
