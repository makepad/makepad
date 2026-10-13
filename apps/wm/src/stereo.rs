//! The WM's VR mode: the live windows of a screen as panels in 3D.
//!
//! Entering VR mode (SUPER+ALT+V, or "VR Mode" in the menu's Style section)
//! takes the windows of the screen the person is on and draws each as a
//! quad at its own depth (SUPER+ALT+PageUp/PageDown). A shared desktop
//! camera preserves their layout on the monitor and in both eyes, with
//! binocular parallax separating the layers. The camera is fixed on the
//! whole source screen (no zoom or navigation); windows sit at their
//! spacewarp poses. The IMU follows small head movements around that view.
//! The windows' own window captures (their chrome and the child's live
//! texture) render under ONE side-by-side pass; each eye draws them through
//! its own projection; a lens pass writes the pair out.
//!
//! Output: the headset's slice of the wide Linux direct desktop when the
//! headset is a live screen (`screen = auto` finds a Beyond or XREAL One Pro by
//! EDID, or a connector name), where the windows also stay on their own
//! screen flat. A missing headset keeps the shared 2D view; explicit
//! `screen = preview` shows the SBS pair on an ordinary display instead.
//! Leaving VR mode
//! draws the desktop as before; the layout is never touched by either.
//!
//! Pointer input is mapped back through the same desktop camera. Over the VR output the flat cursor is
//! hidden and a marker is drawn on the window plane under it.
//!
//! Optics: the Beyond supplies factory calibration; XREAL One Pro uses a
//! nominal 57-degree diagonal field of view and identity distortion. IPD
//! and manual optics overrides live in `$XDG_CONFIG_HOME/makepad/wm/stereo`.
//! A preview on an ordinary monitor uses adjustable, uncalibrated defaults.
//!
//! Head pose: `tracking::HeadTracker` (device independent). Without a
//! tracker the view looks straight ahead and SUPER+SHIFT+CTRL+arrows turn
//! it; SUPER+ALT+Home recenters.

pub mod calibration;
pub mod tracking;
pub mod timewarp;

use std::collections::HashMap;
use std::sync::Arc;
use std::path::PathBuf;

use makepad_widgets::*;

use makepad_app_host::ClientId;
use crate::layout::LRect;
use calibration::{DisplayCalibration, EyeCalibration, LensModel, ValveRadial};
use tracking::{quat_axis_angle, quat_columns, quat_mul, quat_normalize, HeadTracker, TrackedPose, Tracking};
use timewarp::EyeFrame;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.shader.*

    // One window (or the pointer marker) as a quad in 3D, for one eye.
    // The vertex emits homogeneous clip coordinates, so the texture maps
    // perspective-correctly and geometry behind the eye is clipped by the
    // GPU; the pixel discards what falls outside its own eye's frustum
    // (both eyes share the side-by-side pass).
    mod.widgets.DrawStereoWindow = set_type_default() do #(DrawStereoWindow::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        head_rx: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        head_ry: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        head_rz: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        head_pos: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // x: half the eye separation (m)
        eye_params: uniform(vec4(0.0315, 0.0, 0.0, 0.0))
        // Each eye's axes in the head frame (the columns of eye_to_head:
        // canting); identity for parallel eyes.
        eye_lx: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        eye_ly: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        eye_lz: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        eye_rx: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        eye_ry: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        eye_rz: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        // Each eye's off-axis projection: clip x = p.x * x + y * w, clip
        // y = p.y * z + w * w (see EyeCalibration::projection).
        proj_l: uniform(vec4(1.0, 0.0, 1.0, 0.0))
        proj_r: uniform(vec4(1.0, 0.0, 1.0, 0.0))
        // one eye's viewport size in pass units
        eye_view: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        eye_clip: varying(vec2(0.0))
        vertex: fn() {
            let g = self.geom.pos
            let world = self.center.xyz + self.axis_r.xyz * (g.x * 2.0 - 1.0) - self.axis_u.xyz * (g.y * 2.0 - 1.0)
            let rel = world - self.head_pos.xyz
            let h = vec3(dot(self.head_rx.xyz, rel), dot(self.head_ry.xyz, rel), dot(self.head_rz.xyz, rel))
            let v = h - vec3(self.eye_side * self.eye_params.x, 0.0, 0.0)
            let right = step(0.0, self.eye_side)
            let ex = mix(self.eye_lx.xyz, self.eye_rx.xyz, right)
            let ey = mix(self.eye_ly.xyz, self.eye_ry.xyz, right)
            let ez = mix(self.eye_lz.xyz, self.eye_rz.xyz, right)
            let p = vec3(dot(ex, v), dot(ey, v), dot(ez, v))
            let proj = mix(self.proj_l, self.proj_r, right)
            let w = -p.z
            let cx = p.x * proj.x + proj.y * w
            let cy = p.y * proj.z + proj.w * w
            self.pos = g
            self.eye_clip = vec2(cx, w)
            let vx = (self.eye_side * 0.5 + 0.5) * self.eye_view.x
            let hx = (cx * 0.5 + 0.5 * w) * self.eye_view.x + vx * w
            let hy = (0.5 * w - cy * 0.5) * self.eye_view.y
            // The ordinary 2D depth (draw_depth + the call's zbias), scaled
            // by w so the perspective divide gives it back: windows keep
            // makepad's draw-call depth order, and z stays proportional to
            // w, so the near clip removes what lies behind the eye.
            let z = self.draw_depth + self.draw_call.zbias
            self.world = vec4(0.0, 0.0, 0.0, 1.0)
            self.vertex_pos = self.draw_pass.camera_projection * (self.draw_pass.camera_view * vec4(hx, hy, z * w, w))
        }
        fragment: fn() {
            self.fb0 = self.pixel()
        }
        pixel: fn() {
            if abs(self.eye_clip.x) > self.eye_clip.y { discard() }
            if self.color.w > 0.0 {
                let d = length(self.pos - vec2(0.5, 0.5)) * 2.0
                let a = 1.0 - smoothstep(0.8, 1.0, d)
                let core = 1.0 - smoothstep(0.45, 0.6, d)
                let rgb = mix(vec3(0.02, 0.02, 0.02), self.color.rgb, core)
                return vec4(rgb * a * self.color.w, a * self.color.w)
            }
            return self.image.sample(self.pos)
        }
    }

    // The side-by-side eye images into the output, per eye: the headset's
    // factory lens model (calibration::LensModel::ValvePoly3, exactly as
    // Monado's u_compute_distortion_vive) on a calibrated headset, else the
    // generic radial pre-warp (identity unless configured: UNCALIBRATED).
    mod.widgets.DrawStereoLens = set_type_default() do #(DrawStereoLens::script_shader(vm)) {
        ..mod.draw.DrawQuad
        image: texture_2d(float)
        // Inverse rotational timewarp: a ray in the display eye maps into
        // the already rendered eye image. Identity until the late latch.
        tw_lx: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        tw_ly: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        tw_lz: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        tw_rx: uniform(vec4(1.0, 0.0, 0.0, 0.0))
        tw_ry: uniform(vec4(0.0, 1.0, 0.0, 0.0))
        tw_rz: uniform(vec4(0.0, 0.0, 1.0, 0.0))
        tw_src_l: uniform(vec4(1.0, 0.0, 1.0, 0.0))
        tw_src_r: uniform(vec4(1.0, 0.0, 1.0, 0.0))
        tw_dst_l: uniform(vec4(1.0, 0.0, 1.0, 0.0))
        tw_dst_r: uniform(vec4(1.0, 0.0, 1.0, 0.0))
        // x > 0.5: the Valve model with the per-eye values below
        lens_mode: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // per channel r, g, b: 1 = DPOLY3 (divided), 0 = POLY3
        vl_div: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vr_div: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // C0..C2 per channel (w unused), left eye then right
        vl_kr: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vl_kg: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vl_kb: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vr_kr: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vr_kg: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vr_kb: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        // centres: (red.xy, green.xy), (blue.xy, grow_for_undistort, aspect_x_over_y)
        vl_c0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vl_c1: uniform(vec4(0.0, 0.0, 0.5, 0.9))
        vr_c0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        vr_c1: uniform(vec4(0.0, 0.0, 0.5, 0.9))
        // Display (u right, v down) to rendered-image coordinates for one
        // channel; `ga` = (grow_for_undistort, aspect_x_over_y), `div` 1
        // for DPOLY3 (calibration::LensModel::ValveRadial).
        valve: fn(local: vec2, center: vec2, k: vec4, ga: vec2, div: float) -> vec2 {
            let f = 0.5 / (1.0 + ga.x)
            let t = vec2(2.0 * local.x - 1.0, (2.0 * local.y - 1.0) / ga.y) - center
            let r2 = dot(t, t)
            let poly = 1.0 + r2 * (k.x + r2 * (k.y + r2 * k.z))
            let d = mix(poly, 1.0 / poly, div)
            return vec2(0.5, 0.5) + (t * d + center) * vec2(f, f * ga.y)
        }
        // k1, k2, lens centre toward the nose, eye aspect (w / h)
        lens: uniform(vec4(0.0, 0.0, 0.0, 1.0))
        // red and blue scale against green (chromatic aberration)
        chroma: uniform(vec4(1.0, 1.0, 0.0, 0.0))
        warp: fn(local: vec2, center: vec2, scale: float) -> vec2 {
            let d = (local - center) * vec2(self.lens.w, 1.0) * 2.0
            let r2 = dot(d, d)
            let k = (1.0 + self.lens.x * r2 + self.lens.y * r2 * r2) * scale
            return center + (local - center) * k
        }
        reproject: fn(uv: vec2, side: float) -> vec2 {
            let dst = mix(self.tw_dst_l, self.tw_dst_r, side)
            let ray = vec3((uv.x * 2.0 - 1.0 - dst.y) / dst.x,
                (1.0 - uv.y * 2.0 - dst.w) / dst.z, -1.0)
            let r = mix(self.tw_lx.xyz, self.tw_rx.xyz, side) * ray.x
                + mix(self.tw_ly.xyz, self.tw_ry.xyz, side) * ray.y
                + mix(self.tw_lz.xyz, self.tw_rz.xyz, side) * ray.z
            if r.z >= 0.0 { return vec2(-1.0, -1.0) }
            let src = mix(self.tw_src_l, self.tw_src_r, side)
            return vec2((src.x * r.x / -r.z + src.y + 1.0) * 0.5,
                (1.0 - src.z * r.y / -r.z - src.w) * 0.5)
        }
        take: fn(uv: vec2, side: float) -> vec4 {
            let src = self.reproject(uv, side)
            if src.x < 0.0 || src.x > 1.0 || src.y < 0.0 || src.y > 1.0 {
                return vec4(0.0, 0.0, 0.0, 1.0)
            }
            return self.image.sample(vec2((src.x + side) * 0.5, src.y))
        }
        pixel: fn() {
            let side = step(0.5, self.pos.x)
            let local = vec2(self.pos.x * 2.0 - side, self.pos.y)
            if self.lens_mode.x > 0.5 {
                let c0 = mix(self.vl_c0, self.vr_c0, side)
                let c1 = mix(self.vl_c1, self.vr_c1, side)
                let div = mix(self.vl_div, self.vr_div, side)
                let vr = self.take(self.valve(local, c0.xy, mix(self.vl_kr, self.vr_kr, side), c1.zw, div.x), side)
                let vg = self.take(self.valve(local, c0.zw, mix(self.vl_kg, self.vr_kg, side), c1.zw, div.y), side)
                let vb = self.take(self.valve(local, c1.xy, mix(self.vl_kb, self.vr_kb, side), c1.zw, div.z), side)
                return vec4(vr.r, vg.g, vb.b, 1.0)
            }
            // The nose is +x for the left eye, -x for the right.
            let nose = 1.0 - side * 2.0
            let center = vec2(0.5 + nose * self.lens.z * 0.5, 0.5)
            let g = self.take(self.warp(local, center, 1.0), side)
            let r = self.take(self.warp(local, center, self.chroma.x), side)
            let b = self.take(self.warp(local, center, self.chroma.y), side)
            return vec4(r.r, g.g, b.b, 1.0)
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStereoWindow {
    #[deref]
    draw_super: DrawQuad,
    #[live]
    center: Vec4f,
    #[live]
    axis_r: Vec4f,
    #[live]
    axis_u: Vec4f,
    /// The pointer marker's colour; zero alpha draws the texture.
    #[live]
    pub color: Vec4f,
    /// -1 left eye, +1 right eye.
    #[live]
    eye_side: f32,
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawStereoLens {
    #[deref]
    draw_super: DrawQuad,
}

// ======================================================================
// Configuration
// ======================================================================

/// Which connector is the headset.
#[derive(Clone, Debug, PartialEq)]
pub enum HeadsetScreen {
    /// The connector whose EDID names a supported headset.
    Auto,
    Named(String),
}

/// The view's numbers. On a headset whose backend reports its optics
/// (`calibration::DisplayCalibration`), those replace the field of view,
/// lens centre and pre-warp below and give the default eye separation; the
/// side-by-side preview on an ordinary screen always uses these defaults,
/// none of which is a measured property of a headset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoParams {
    /// Apply the measured head pose to rendering (`head_tracking = on | off`).
    /// Off keeps a fixed view while the device still supplies stereo control
    /// and sensor diagnostics.
    pub head_tracking: bool,
    /// Optional look-ahead from now, milliseconds, in addition to sample age.
    /// Unset uses the display's scanout estimate and reported panel delay.
    pub prediction_ms: Option<f32>,
    /// Eye separation, metres, when the person set one (`ipd_mm`); it wins
    /// over a headset's lens separation. Unset: the headset's lens
    /// separation, else `DEFAULT_IPD`.
    pub ipd: Option<f32>,
    /// Use a headset's reported optics on its own screen
    /// (`calibration = factory | off`).
    pub use_calibration: bool,
    /// Each eye's horizontal field of view, degrees (`fov_deg`); the
    /// vertical one follows the eye viewport's aspect. Uncalibrated only.
    pub fov_deg: f32,
    /// Lens centre's shift toward the nose, as a fraction of half the eye
    /// width (`lens_center`).
    pub lens_center: f32,
    /// Radial pre-warp coefficients (`lens_k1`, `lens_k2`).
    pub k1: f32,
    pub k2: f32,
    /// Red and blue radial scale against green (`lens_ca_red`,
    /// `lens_ca_blue`).
    pub chroma: [f32; 2],
    /// Nearest and farthest depth a window may take, metres.
    pub depth_min: f32,
    pub depth_max: f32,
    /// The scene's background (`background`, `#rrggbb`).
    pub background: [f32; 3],
}

impl Default for StereoParams {
    fn default() -> Self {
        Self {
            head_tracking: true,
            prediction_ms: None,
            ipd: None,
            use_calibration: true,
            fov_deg: 90.0,
            lens_center: 0.0,
            k1: 0.0,
            k2: 0.0,
            chroma: [1.0, 1.0],
            depth_min: 0.4,
            depth_max: 20.0,
            background: [0.03, 0.035, 0.045],
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StereoConfig {
    /// The headset output; `None` keeps VR mode in preview on the screen
    /// itself.
    pub screen: Option<HeadsetScreen>,
    pub params: StereoParams,
    /// Depths, metres, handed to windows as they arrive on the headset,
    /// in turn (`depths = 1.2 2.4 3.6`).
    pub depth_slots: Vec<f32>,
}

const CONFIG_ENV: &str = "MAKEPAD_WM_STEREO";
const CONFIG_FILE: &str = "stereo";
const CONFIG_MAX_LEN: u64 = 16 << 10;

impl StereoConfig {
    /// The file's settings over the defaults; `MAKEPAD_WM_STEREO` names the
    /// headset screen. The default headset screen is `auto`.
    pub fn load() -> Self {
        let text = config_path()
            .and_then(|path| {
                use std::io::Read;
                let file = std::fs::File::open(path).ok()?;
                let mut text = String::new();
                file.take(CONFIG_MAX_LEN).read_to_string(&mut text).ok()?;
                Some(text)
            })
            .unwrap_or_default();
        let env = std::env::var(CONFIG_ENV).ok();
        let config = Self::parse(&text, env.as_deref());
        log!(
            "wm: vr headset screen {:?}, {:?} (uncalibrated defaults unless set)",
            config.screen,
            config.params
        );
        config
    }

    /// `text` is the file, `env` the variable's value (which wins for the
    /// screen; `preview` means none). Unknown keys and bad values are
    /// logged and skipped.
    pub fn parse(text: &str, env: Option<&str>) -> Self {
        let mut screen = Some(HeadsetScreen::Auto);
        let mut params = StereoParams::default();
        let mut depth_slots = vec![1.2, 2.4, 3.6];
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                log!("wm: stereo config: ignoring '{}'", line);
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            let num = |lo: f32, hi: f32| value.parse::<f32>().ok().filter(|v| v.is_finite() && *v >= lo && *v <= hi);
            let ok = match key {
                "screen" => {
                    screen = parse_screen(value);
                    true
                }
                "ipd_mm" => num(40.0, 85.0).map(|v| params.ipd = Some(v / 1000.0)).is_some(),
                "head_tracking" => match value {
                    "on" | "true" | "1" => { params.head_tracking = true; true }
                    "off" | "false" | "0" => { params.head_tracking = false; true }
                    _ => false,
                },

                "prediction_ms" => {
                    if value == "auto" { params.prediction_ms = None; true }
                    else {
                        value.parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0)
                            .map(|v| params.prediction_ms = Some(v)).is_some()
                    }
                },
                "calibration" => match value {
                    "factory" | "on" => {
                        params.use_calibration = true;
                        true
                    }
                    "off" => {
                        params.use_calibration = false;
                        true
                    }
                    _ => false,
                },
                "fov_deg" => num(30.0, 170.0).map(|v| params.fov_deg = v).is_some(),
                "lens_center" => num(-1.0, 1.0).map(|v| params.lens_center = v).is_some(),
                "lens_k1" => num(-2.0, 2.0).map(|v| params.k1 = v).is_some(),
                "lens_k2" => num(-2.0, 2.0).map(|v| params.k2 = v).is_some(),
                "lens_ca_red" => num(0.8, 1.2).map(|v| params.chroma[0] = v).is_some(),
                "lens_ca_blue" => num(0.8, 1.2).map(|v| params.chroma[1] = v).is_some(),
                "depth_min" => num(0.1, 10.0).map(|v| params.depth_min = v).is_some(),
                "depth_max" => num(0.5, 100.0).map(|v| params.depth_max = v).is_some(),
                "background" => parse_hex_rgb(value).map(|v| params.background = v).is_some(),
                "depths" => {
                    let slots: Vec<f32> = value
                        .split_whitespace()
                        .filter_map(|v| v.parse::<f32>().ok())
                        .filter(|v| v.is_finite() && *v > 0.0)
                        .collect();
                    let ok = !slots.is_empty();
                    if ok {
                        depth_slots = slots;
                    }
                    ok
                }
                _ => false,
            };
            if !ok {
                log!("wm: stereo config: ignoring '{}'", line);
            }
        }
        if let Some(env) = env {
            screen = parse_screen(env.trim());
        }
        if params.depth_max < params.depth_min {
            params.depth_max = params.depth_min;
        }
        for slot in &mut depth_slots {
            *slot = slot.clamp(params.depth_min, params.depth_max);
        }
        Self { screen, params, depth_slots }
    }
}

fn parse_screen(value: &str) -> Option<HeadsetScreen> {
    match value {
        "" | "none" | "preview" | "off" | "0" => None,
        "auto" | "1" | "on" => Some(HeadsetScreen::Auto),
        name => Some(HeadsetScreen::Named(name.to_string())),
    }
}

fn parse_hex_rgb(value: &str) -> Option<[f32; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some([
        ((v >> 16) & 255) as f32 / 255.0,
        ((v >> 8) & 255) as f32 / 255.0,
        (v & 255) as f32 / 255.0,
    ])
}

/// `$XDG_CONFIG_HOME/makepad/wm/stereo` when that is absolute, else
/// `$HOME/.config/makepad/wm/stereo`.
fn config_path() -> Option<PathBuf> {
    let absolute = |key: &str| std::env::var_os(key).map(PathBuf::from).filter(|p| p.is_absolute());
    let root = absolute("XDG_CONFIG_HOME").or_else(|| absolute("HOME").map(|h| h.join(".config")))?;
    Some(root.join("makepad").join("wm").join(CONFIG_FILE))
}

/// The Bigscreen Beyond's EDID identity: manufacturer `BIG`, product 0x1234.
const BEYOND_EDID: ([u8; 3], u16) = (*b"BIG", 0x1234);
pub const XREAL_ONE_PRO_EDID: ([u8; 3], u16) = (*b"MRG", 0x4100);

/// The manufacturer letters and product code of an EDID base block.
pub fn edid_identity(edid: &[u8]) -> Option<([u8; 3], u16)> {
    if edid.len() < 12 || edid[..8] != [0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00] {
        return None;
    }
    let m = u16::from_be_bytes([edid[8], edid[9]]);
    let letter = |shift: u16| b'A' - 1 + ((m >> shift) & 31) as u8;
    let product = u16::from_le_bytes([edid[10], edid[11]]);
    Some(([letter(10), letter(5), letter(0)], product))
}

/// The connector's EDID identity from sysfs (Linux), `None` without one.
fn connector_identity(name: &str) -> Option<([u8; 3], u16)> {
    use std::io::Read;
    if name.contains('/') || name.contains("..") {
        return None;
    }
    let file = std::fs::File::open(format!("/sys/class/drm/{name}/edid")).ok()?;
    let mut edid = Vec::with_capacity(256);
    file.take(4096).read_to_end(&mut edid).ok()?;
    edid_identity(&edid)
}

fn connector_is_headset(name: &str) -> bool {
    connector_identity(name).is_some_and(|id| id == BEYOND_EDID || id == XREAL_ONE_PRO_EDID)
}

// ======================================================================
// The WM-side state (actions write it, the desk reads it)
// ======================================================================

/// VR mode as `WmState` keeps it: on or off, which screen's windows it
/// shows, each window's depth, the manual look and recenter requests.
pub struct VrState {
    pub config: StereoConfig,
    /// VR mode is on, showing this screen's windows (its connector name,
    /// `""` for the single desk).
    pub source: Option<String>,
    /// The headset connector among the screens, resolved for the last
    /// screen-name list (`resolve_headset`), and its EDID manufacturer.
    resolved: (Vec<String>, Option<String>),
    display_generation: u64,
    headset_identity: Option<([u8; 3], u16)>,
    depths: HashMap<ClientId, f32>,
    next_slot: usize,
    /// Manual look (radians, right and up positive), on top of the tracked
    /// pose: how to turn the view without a tracker.
    pub look: (f32, f32),
    /// Bumped by the recenter action; the desk recenters the tracker when
    /// it sees a new value.
    pub recenter: u32,
    /// Where the desk last drew the VR output (the headset's slice, or the
    /// source screen in preview): the flat cursor is hidden over it.
    pub output: Option<LRect>,
    /// The pointer, while VR mode is on (the marker follows it).
    pub pointer: Option<(f64, f64)>,
    pub physical_pointer: Option<(f64, f64)>,
    /// The WM hid the flat cursor over the output and owes it back.
    pub cursor_hidden: bool,
}

impl VrState {
    pub fn load() -> Self {
        Self {
            config: StereoConfig::load(),
            source: None,
            resolved: (Vec::new(), None),
            display_generation: u64::MAX,
            headset_identity: None,
            depths: HashMap::new(),
            next_slot: 0,
            look: (0.0, 0.0),
            recenter: 0,
            output: None,
            pointer: None,
            physical_pointer: None,
            cursor_hidden: false,
        }
    }

    /// The flat cursor is hidden at `(x, y)`: VR mode draws its own marker.
    pub fn hides_cursor(&self, x: f64, y: f64) -> bool {
        self.output.is_some_and(|r| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
    }

    pub fn active(&self) -> bool {
        self.source.is_some()
    }

    /// The headset's connector among `names`, or `None` (no headset output
    /// configured, or no such screen). EDIDs are read on display generation
    /// changes too: a different headset can occupy the same connector.
    pub fn resolve_headset(&mut self, names: &[String], generation: u64) -> Option<String> {
        if self.resolved.0 != names || self.display_generation != generation {
            let found = match &self.config.screen {
                None => None,
                Some(HeadsetScreen::Named(name)) => names.iter().find(|n| *n == name).cloned(),
                Some(HeadsetScreen::Auto) => names.iter().find(|n| connector_is_headset(n)).cloned(),
            };
            if found != self.resolved.1 {
                log!("wm: vr headset screen {:?}", found);
            }
            self.headset_identity = found.as_deref().and_then(connector_identity);
            self.display_generation = generation;
            self.resolved = (names.to_vec(), found);
        }
        self.resolved.1.clone()
    }

    /// The headset connector from the last `resolve_headset`.
    pub fn headset(&self) -> Option<&str> {
        self.resolved.1.as_deref()
    }

    pub fn headset_identity(&self) -> Option<([u8; 3], u16)> {
        self.headset_identity
    }

    /// `client`'s depth, metres; a window new to VR takes the next slot.
    pub fn depth_of(&mut self, client: ClientId) -> f32 {
        if let Some(depth) = self.depths.get(&client) {
            return *depth;
        }
        let slots = &self.config.depth_slots;
        let depth = slots[self.next_slot % slots.len()];
        self.next_slot += 1;
        self.depths.insert(client, depth);
        depth
    }

    /// Move `client` nearer (`factor` < 1) or farther, within range.
    pub fn scale_depth(&mut self, client: ClientId, factor: f32) -> f32 {
        let p = self.config.params;
        let depth = (self.depth_of(client) * factor).clamp(p.depth_min, p.depth_max);
        self.depths.insert(client, depth);
        depth
    }

    /// Forget windows that are gone.
    pub fn retain(&mut self, live: impl Fn(ClientId) -> bool) {
        self.depths.retain(|c, _| live(*c));
    }
}

// ======================================================================
// Geometry
// ======================================================================

/// A common desktop camera. The monitor maps this rectangle directly;
/// the headset projects the same rectangles at their individual depths.
/// Scaling each plane by its depth preserves the 2D layout in the centre
/// eye; only binocular parallax separates the layers.
#[derive(Clone, Copy, Debug)]
pub struct DesktopView {
    pub visible: LRect,
    pub mirror: Rect,
    tangent: [f32; 2],
    rotation: [f32; 4],
}

impl DesktopView {
    pub fn map_rect(&self, rect: Rect) -> Rect {
        let scale = self.mirror.size.x / self.visible.w;
        Rect {
            pos: self.mirror.pos + dvec2(rect.pos.x - self.visible.x, rect.pos.y - self.visible.y) * scale,
            size: rect.size * scale,
        }
    }

    pub fn unmap(&self, point: Vec2d, viewport: Rect) -> Vec2d {
        dvec2(
            self.visible.x + (point.x - viewport.pos.x) / viewport.size.x * self.visible.w,
            self.visible.y + (point.y - viewport.pos.y) / viewport.size.y * self.visible.h,
        )
    }

    pub fn place(&self, rect: Rect, depth: f32) -> Placement {
        let sx = 2.0 * self.tangent[0] * depth / self.visible.w as f32;
        let sy = 2.0 * self.tangent[1] * depth / self.visible.h as f32;
        let axes = quat_columns(self.rotation);
        let rotate = |v: [f32; 3]| [0, 1, 2].map(|i| axes[0][i] * v[0] + axes[1][i] * v[1] + axes[2][i] * v[2]);
        Placement {
            center: rotate([
                (rect.pos.x + rect.size.x * 0.5 - self.visible.x - self.visible.w * 0.5) as f32 * sx,
                -(rect.pos.y + rect.size.y * 0.5 - self.visible.y - self.visible.h * 0.5) as f32 * sy,
                -depth,
            ]),
            right: rotate([rect.size.x as f32 * sx * 0.5, 0.0, 0.0]),
            up: rotate([0.0, rect.size.y as f32 * sy * 0.5, 0.0]),
        }
    }
}

/// A window's quad in world space: its centre and half-extent axes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub center: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
}

impl Placement {
    /// The world point at `(u, v)` (0..1, v down) on the quad.
    pub fn point(&self, u: f32, v: f32) -> [f32; 3] {
        let (a, b) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
        [0, 1, 2].map(|i| self.center[i] + self.right[i] * a - self.up[i] * b)
    }
}

/// The marker for a pointer at `(u, v)` on `on`: a small disc just in front
/// of the plane, about 0.8 degrees across.
pub fn pointer_marker(on: &Placement, u: f32, v: f32) -> Placement {
    let p = on.point(u, v);
    let dist = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(0.01);
    let scale = 0.996;
    let size = dist * 0.007;
    let unit = |a: [f32; 3]| {
        let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt().max(1e-6);
        a.map(|x| x / n * size)
    };
    Placement {
        center: p.map(|x| x * scale),
        right: unit(on.right),
        up: unit(on.up),
    }
}

// ======================================================================
// Optics: calibrated headset or uncalibrated defaults, as uniforms
// ======================================================================

/// The eye separation when neither the person nor the headset gives one.
pub const DEFAULT_IPD: f32 = 0.063;

/// One frame's optics, ready as shader uniforms.
#[derive(Clone, Debug, PartialEq)]
pub struct Optics {
    pub half_separation: f32,
    /// Per eye (left, right): the eye's axes in the head frame.
    eye_axes: [[[f32; 3]; 3]; 2],
    /// Per eye: (x scale, x offset, y scale, y offset).
    projection: [[f32; 4]; 2],
    /// `None`: the generic pre-warp from `StereoParams`.
    valve: Option<[[f32; 4]; 12]>,
    /// An identity headset profile must not inherit manual radial warp
    /// settings belonging to a different headset.
    identity_lens: bool,
}

impl Optics {
    pub fn new(params: &StereoParams, calibration: Option<&DisplayCalibration>, size: Vec2d) -> Self {
        let separation = params
            .ipd
            .or_else(|| calibration.and_then(|c| c.lens_separation))
            .unwrap_or(DEFAULT_IPD);
        match calibration {
            Some(c) => {
                // Columns of eye_to_head = the eye's axes in the head frame.
                let axes = |e: &EyeCalibration| {
                    let m = e.eye_to_head;
                    [0, 1, 2].map(|col| [m[0][col], m[1][col], m[2][col]])
                };
                let valve = match (&c.eyes[0].lens, &c.eyes[1].lens) {
                    (
                        LensModel::ValveRadial { radial: tl, center: cl, k: kl, grow_for_undistort: gl, aspect_x_over_y: al },
                        LensModel::ValveRadial { radial: tr, center: cr, k: kr, grow_for_undistort: gr, aspect_x_over_y: ar },
                    ) => {
                        let k4 = |k: [f32; 3]| [k[0], k[1], k[2], 0.0];
                        let div = |t: [ValveRadial; 3]| t.map(|t| if t == ValveRadial::DPoly3 { 1.0 } else { 0.0 });
                        let (dl, dr) = (div(*tl), div(*tr));
                        Some([
                            k4(kl[0]),
                            k4(kl[1]),
                            k4(kl[2]),
                            k4(kr[0]),
                            k4(kr[1]),
                            k4(kr[2]),
                            [cl[0][0], cl[0][1], cl[1][0], cl[1][1]],
                            [cl[2][0], cl[2][1], *gl, *al],
                            [cr[0][0], cr[0][1], cr[1][0], cr[1][1]],
                            [cr[2][0], cr[2][1], *gr, *ar],
                            [dl[0], dl[1], dl[2], 0.0],
                            [dr[0], dr[1], dr[2], 0.0],
                        ])
                    }
                    // An eye without a known lens model: both stay as rendered.
                    _ => None,
                };
                Self {
                    half_separation: separation * 0.5,
                    eye_axes: [axes(&c.eyes[0]), axes(&c.eyes[1])],
                    projection: [c.eyes[0].projection(), c.eyes[1].projection()],
                    valve,
                    identity_lens: matches!((&c.eyes[0].lens, &c.eyes[1].lens), (LensModel::Identity, LensModel::Identity)),
                }
            }
            None => {
                let eye_w = (size.x * 0.5).max(1.0) as f32;
                let eye_h = size.y.max(1.0) as f32;
                let sx = 1.0 / (params.fov_deg.to_radians() * 0.5).tan();
                let sy = sx * eye_w / eye_h;
                // The lens centre moves toward the nose: +x for the left eye.
                let shift = params.lens_center;
                let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
                Self {
                    half_separation: separation * 0.5,
                    eye_axes: [identity, identity],
                    projection: [[sx, shift, sy, 0.0], [sx, -shift, sy, 0.0]],
                    valve: None,
                    identity_lens: false,
                }
            }
        }
    }

    /// Eye separation, axes and projections for `DrawStereoWindow`.
    fn eye_uniforms(&self) -> [(LiveId, [f32; 4]); 9] {
        let a = |eye: usize, axis: usize| {
            let v = self.eye_axes[eye][axis];
            [v[0], v[1], v[2], 0.0]
        };
        [
            (live_id!(eye_params), [self.half_separation, 0.0, 0.0, 0.0]),
            (live_id!(eye_lx), a(0, 0)),
            (live_id!(eye_ly), a(0, 1)),
            (live_id!(eye_lz), a(0, 2)),
            (live_id!(eye_rx), a(1, 0)),
            (live_id!(eye_ry), a(1, 1)),
            (live_id!(eye_rz), a(1, 2)),
            (live_id!(proj_l), self.projection[0]),
            (live_id!(proj_r), self.projection[1]),
        ]
    }

    /// The lens pass's uniforms: the headset's lens model, or the generic
    /// pre-warp of `params` for an eye `size` (pass units, both eyes).
    pub fn lens_uniforms(&self, params: &StereoParams, size: Vec2d) -> [(LiveId, [f32; 4]); 15] {
        let aspect = ((size.x * 0.5) / size.y.max(1.0)) as f32;
        let v = self.valve.unwrap_or([[0.0; 4]; 12]);
        let lens = if self.identity_lens { [0.0, 0.0, 0.0, aspect] } else { [params.k1, params.k2, params.lens_center, aspect] };
        let chroma = if self.identity_lens { [1.0, 1.0, 0.0, 0.0] } else { [params.chroma[0], params.chroma[1], 0.0, 0.0] };
        [
            (live_id!(lens_mode), [if self.valve.is_some() { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0]),
            (live_id!(lens), lens),
            (live_id!(chroma), chroma),
            (live_id!(vl_kr), v[0]),
            (live_id!(vl_kg), v[1]),
            (live_id!(vl_kb), v[2]),
            (live_id!(vr_kr), v[3]),
            (live_id!(vr_kg), v[4]),
            (live_id!(vr_kb), v[5]),
            (live_id!(vl_c0), v[6]),
            (live_id!(vl_c1), v[7]),
            (live_id!(vr_c0), v[8]),
            (live_id!(vr_c1), v[9]),
            (live_id!(vl_div), v[10]),
            (live_id!(vr_div), v[11]),
        ]
    }
}

// ======================================================================
// The view (owned by the desk while VR mode is on)
// ======================================================================

/// The side-by-side pass, the tracker and the pose last drawn with.
struct DesktopAnchor {
    recenter: u32,
    rotation: [f32; 4],
    tracked: bool,
}

pub struct StereoView {
    pass: DrawPass,
    list: DrawList2d,
    texture: Texture,
    _depth: Texture,
    /// The retained window draw calls, for pose updates without a redraw.
    areas: Vec<Area>,
    tracker: HeadTracker,
    pose: TrackedPose,
    head_tracking: bool,
    prediction_ms: Option<f32>,
    recenter_seen: u32,
    look: (f32, f32),
    /// Every frame while tracked; a slow poll while not.
    next: NextFrame,
    poll: Timer,
    /// The clear colour the pass was last given.
    background: Option<[f32; 3]>,
    /// The optics choice last logged: (headset output, which calibration,
    /// half separation).
    optics_logged: Option<(bool, Option<usize>, f32)>,
    /// The pointer marker: its two eye instances, and what it lands on
    /// (window rects in desk points with their quads, nearest first; the
    /// backdrop plane over the source screen), so a pointer move rewrites
    /// two instances instead of redrawing the desk.
    marker: [Area; 2],
    marker_targets: Vec<(Rect, Placement)>,
    marker_source: LRect,
    marker_backdrop: Option<Placement>,
    /// Where the fixed desktop camera was anchored (see `desktop_view`).
    anchor: Option<DesktopAnchor>,
}

/// How often an untracked view looks for a tracker, seconds.
const TRACKER_POLL: f64 = 0.5;

impl StereoView {
    /// Starts the trackers (their workers); dropping the view stops them.
    pub fn new(cx: &mut Cx, recenter_seen: u32, display: Option<([u8; 3], u16)>) -> Self {
        let pass = DrawPass::new_with_name(cx, "wm_vr");
        let texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 { size: TextureSize::Auto, initial: true },
        );
        let depth = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 { size: TextureSize::Auto, initial: true },
        );
        pass.set_depth_texture(cx, &depth, DrawPassClearDepth::ClearWith(1.0));
        let tracker = HeadTracker::start(cx, display);
        let poll = cx.start_timeout(TRACKER_POLL);
        Self {
            pass,
            list: DrawList2d::new(cx),
            texture,
            _depth: depth,
            areas: Vec::new(),
            tracker,
            pose: TrackedPose::default(),
            head_tracking: true,
            prediction_ms: None,
            recenter_seen,
            look: (0.0, 0.0),
            next: NextFrame::default(),
            poll,
            background: None,
            optics_logged: None,
            marker: [Area::Empty; 2],
            marker_targets: Vec::new(),
            marker_source: LRect::new(0.0, 0.0, 0.0, 0.0),
            marker_backdrop: None,
            anchor: None,
        }
    }

    pub fn texture(&self) -> &Texture {
        &self.texture
    }

    pub fn tracker_status(&self) -> Vec<String> {
        self.tracker.status()
    }

    /// The fixed desktop camera: the whole source screen, with room for
    /// stereo parallax and a little context around it. It never moves or
    /// zooms; windows are placed by their spacewarp poses. Its heading is
    /// anchored where the person looked when it was first made (and again on
    /// a recenter, or when a tracker first reports), and afterwards stays put
    /// while the IMU follows their head.
    pub fn desktop_view(&mut self, source: LRect, mirror_source: LRect, optics: &Optics) -> DesktopView {
        let tangent = [0, 1].map(|axis| optics.projection.iter()
            .map(|p| (1.0 - p[axis * 2 + 1].abs()).max(0.1) / p[axis * 2])
            .fold(f32::INFINITY, f32::min));
        let aspect = (tangent[0] / tangent[1]) as f64;
        let width = source.w.max(source.h * aspect).max(1.0) * 1.16;
        let height = width / aspect;
        let visible = LRect::new(source.x + (source.w - width) * 0.5, source.y + (source.h - height) * 0.5, width, height);
        let rotation = self.head_rotation();
        let tracked = self.pose.tracking != Tracking::None;
        let anchor = self.anchor.get_or_insert(DesktopAnchor { recenter: self.recenter_seen, rotation, tracked });
        if anchor.recenter != self.recenter_seen || (!anchor.tracked && tracked) {
            anchor.rotation = rotation;
        }
        anchor.recenter = self.recenter_seen;
        anchor.tracked = tracked;
        let w = mirror_source.w.min(mirror_source.h * aspect);
        let h = w / aspect;
        DesktopView {
            visible,
            mirror: Rect { pos: dvec2(mirror_source.x + (mirror_source.w - w) * 0.5, mirror_source.y + (mirror_source.h - h) * 0.5), size: dvec2(w, h) },
            tangent, rotation: anchor.rotation,
        }
    }

    /// Start the side-by-side pass of `size`. Window captures begun until
    /// `end` become its children, so they render before it and their new
    /// frames repaint it.
    pub fn begin(&mut self, cx: &mut Cx2d, size: Vec2d, background: [f32; 3]) {
        if self.background != Some(background) {
            self.background = Some(background);
            let [r, g, b] = background;
            self.pass.set_color_texture(cx, &self.texture, DrawPassClearColor::ClearWith(vec4(r, g, b, 1.0)));
        }
        let dpi = cx.current_dpi_factor();
        self.pass.set_size(cx, size);
        cx.set_pass_shift_scale(&self.pass, dvec2(0.0, 0.0), dvec2(1.0, 1.0));
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(dpi));
        self.list.begin_always(cx);
        cx.begin_root_turtle(size, Layout::flow_overlay());
        self.areas.clear();
    }

    pub fn end(&mut self, cx: &mut Cx2d) {
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
    }

    /// Take the tracker's newest pose, the manual look and a recenter
    /// request. True when the head changed.
    fn refresh_pose(&mut self, cx: &Cx, look: (f32, f32), recenter: u32, headset: Option<&str>) -> bool {
        if recenter != self.recenter_seen {
            self.recenter_seen = recenter;
            self.tracker.recenter();
        }
        let pose = if self.head_tracking {
            let now = Cx::monotonic_now();
            let display_time = match self.prediction_ms.filter(|_| headset.is_some()) {
                Some(ms) => now + ms as f64 * 0.001,
                None => predicted_display_time(cx, headset, now, self.tracker.vsync_to_photons()),
            };
            self.tracker.pose(now, display_time)
        } else {
            TrackedPose::default()
        };
        let changed = pose != self.pose || look != self.look;
        self.pose = pose;
        self.look = look;
        changed
    }

    pub fn tracking(&self) -> Tracking {
        self.pose.tracking
    }

    /// The head-to-world rotation: the tracked one, then the manual look in
    /// head space.
    fn head_rotation(&self) -> [f32; 4] {
        let look = quat_mul(
            quat_axis_angle([0.0, 1.0, 0.0], -self.look.0),
            quat_axis_angle([1.0, 0.0, 0.0], self.look.1),
        );
        quat_normalize(quat_mul(quat_normalize(self.pose.orientation), look))
    }

    /// The four head uniforms (head axes in world space, head position).
    fn head_uniforms(&self) -> [(LiveId, [f32; 4]); 4] {
        let [r, u, b] = quat_columns(self.head_rotation());
        let p = self.pose.position;
        [
            (live_id!(head_rx), [r[0], r[1], r[2], 0.0]),
            (live_id!(head_ry), [u[0], u[1], u[2], 0.0]),
            (live_id!(head_rz), [b[0], b[1], b[2], 0.0]),
            (live_id!(head_pos), [p[0], p[1], p[2], 0.0]),
        ]
    }

    /// Take the newest pose (once per draw, before `stamp`).
    pub fn prepare(&mut self, cx: &Cx, look: (f32, f32), recenter: u32, headset: Option<&str>, display: Option<([u8; 3], u16)>, params: &StereoParams) {
        self.head_tracking = params.head_tracking;
        self.prediction_ms = params.prediction_ms;
        self.tracker.select_display(display);
        self.refresh_pose(cx, look, recenter, headset);
    }

    /// Do not put an SBS pair into XREAL's mono/half-SBS timing while its
    /// worker configures full SBS. The profile arrives only after control
    /// readback succeeds; the desktop geometry independently proves 32:9.
    pub fn output_ready(&mut self, display: Option<([u8; 3], u16)>, size: Vec2d) -> bool {
        if display != Some(XREAL_ONE_PRO_EDID) { return true; }
        let (_, profile) = self.tracker.display_calibration();
        profile.is_some_and(|p| p.edid_vendor == Some(XREAL_ONE_PRO_EDID.0))
            && (size.x / size.y.max(1.0) - 32.0 / 9.0).abs() < 0.01
    }

    /// Lens correction samples the complete eye pass as a dependency of
    /// the containing pass. Tracker changes request a complete redraw.
    pub fn draw_lens(&mut self, cx: &mut Cx2d, draw: &mut DrawStereoLens, optics: &Optics,
        params: &StereoParams, output: Rect) {
        // The containing pass may differ from the one that was current
        // while rendering the eyes (e.g. a desktop backdrop stack).
        cx.make_child_pass(&self.pass);
        let frame = EyeFrame {
            orientation: self.head_rotation(),
            eye_axes: optics.eye_axes,
            projection: optics.projection,
        };
        draw.draw_vars.set_texture(0, &self.texture);
        for (id, value) in optics.lens_uniforms(params, output.size) {
            draw.draw_vars.set_uniform(cx, id, &value);
        }
        for (id, value) in frame.uniforms(&frame) {
            draw.draw_vars.set_uniform(cx, id, &value);
        }
        draw.draw_abs(cx, output);

    }

    pub fn close(&mut self, cx: &mut Cx) {
        cx.stop_timer(self.poll);
    }

    /// The optics for this frame: the headset's reported calibration when
    /// the output IS that headset (`headset` is its EDID vendor when it is
    /// a headset screen, `None` for the preview), else the uncalibrated
    /// defaults. Logs when the choice changes. Per frame: one atomic load
    /// per tracker source.
    pub fn optics(&mut self, params: &StereoParams, headset: Option<Option<[u8; 3]>>, size: Vec2d) -> Optics {
        let (_, calibration) = self.tracker.display_calibration();
        let calibration = match headset {
            Some(vendor) if params.use_calibration => calibration.filter(|c| match (c.edid_vendor, vendor) {
                (Some(want), Some(have)) => want == have,
                _ => true,
            }),
            _ => None,
        };
        let optics = Optics::new(params, calibration.as_deref(), size);
        // Compared by identity: no allocation per frame.
        let mode = (headset.is_some(), calibration.as_ref().map(|c| Arc::as_ptr(c) as usize), optics.half_separation);
        if self.optics_logged != Some(mode) {
            match calibration.as_ref().map(|c| &c.source) {
                Some(source) => log!(
                    "wm: vr optics: headset profile ({source}), eye separation {:.1} mm{}",
                    optics.half_separation * 2000.0,
                    if params.ipd.is_some() { " (ipd_mm override)" }
                    else if calibration.as_ref().is_some_and(|c| c.lens_separation.is_some()) { " (device lens separation)" }
                    else { " (default; set ipd_mm for your eyes)" }
                ),
                None if mode.0 => log!(
                    "wm: vr optics: headset screen WITHOUT calibration (defaults, uncalibrated), eye separation {:.1} mm",
                    optics.half_separation * 2000.0
                ),
                None => log!(
                    "wm: vr optics: side-by-side preview (uncalibrated defaults), eye separation {:.1} mm",
                    optics.half_separation * 2000.0
                ),
            }
            self.optics_logged = Some(mode);
        }
        optics
    }

    /// Stamp the pose, eye and viewport uniforms on `draw` before drawing
    /// into a pass of `size`.
    pub fn stamp(&self, cx: &Cx, draw: &mut DrawStereoWindow, optics: &Optics, size: Vec2d) {
        for (id, value) in self.head_uniforms() {
            draw.draw_vars.set_uniform(cx, id, &value);
        }
        for (id, value) in optics.eye_uniforms() {
            draw.draw_vars.set_uniform(cx, id, &value);
        }
        let eye_w = (size.x * 0.5).max(1.0) as f32;
        let eye_h = size.y.max(1.0) as f32;
        draw.draw_vars.set_uniform(cx, live_id!(eye_view), &[eye_w, eye_h, 0.0, 0.0]);
    }

    /// Draw `placement` for both eyes with `draw`'s texture (or marker
    /// colour), and keep the draw call for pose updates.
    pub fn draw_quad(&mut self, cx: &mut Cx2d, draw: &mut DrawStereoWindow, placement: &Placement, size: Vec2d) {
        Self::set_quad(draw, placement);
        let local = Rect { pos: dvec2(0.0, 0.0), size };
        for side in [-1.0, 1.0] {
            draw.eye_side = side;
            draw.draw_abs(cx, local);
        }
        let area = draw.draw_vars.area;
        if !self.areas.contains(&area) {
            self.areas.push(area);
        }
    }

    /// Draw the pointer marker (both eyes, last, so it is on top) and keep
    /// what a pointer move needs: `targets` are the windows' desk rects and
    /// quads nearest first, `backdrop` the plane over `source` for a
    /// pointer between windows. Outside `source` it is drawn collapsed.
    pub fn draw_marker(
        &mut self,
        cx: &mut Cx2d,
        draw: &mut DrawStereoWindow,
        size: Vec2d,
        targets: Vec<(Rect, Placement)>,
        source: LRect,
        backdrop: Placement,
        pointer: Option<(f64, f64)>,
    ) {
        self.marker_targets = targets;
        self.marker_source = source;
        self.marker_backdrop = Some(backdrop);
        let placed = self.marker_for(pointer);
        Self::set_quad(draw, &placed);
        draw.color = vec4(0.95, 0.95, 0.97, 1.0);
        // Never sampled (the marker branch returns first), but bound.
        draw.draw_vars.set_texture(0, &cx.null_texture());
        let local = Rect { pos: dvec2(0.0, 0.0), size };
        for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
            draw.eye_side = side;
            draw.draw_abs(cx, local);
            self.marker[i] = draw.draw_vars.area;
        }
        let area = draw.draw_vars.area;
        if !self.areas.contains(&area) {
            self.areas.push(area);
        }
    }

    /// Move the marker to `pointer` by rewriting its two instances: no
    /// redraw, its pass repaints.
    pub fn move_marker(&mut self, cx: &mut Cx, draw: &mut DrawStereoWindow, pointer: Option<(f64, f64)>) {
        let placed = self.marker_for(pointer);
        Self::set_quad(draw, &placed);
        for (i, side) in [-1.0f32, 1.0].into_iter().enumerate() {
            draw.eye_side = side;
            draw.draw_vars.area = self.marker[i];
            for id in [live_id!(center), live_id!(axis_r), live_id!(axis_u)] {
                draw.draw_vars.update_instance_area_value(cx, &[id]);
            }
        }
    }

    fn marker_for(&self, pointer: Option<(f64, f64)>) -> Placement {
        let hidden = Placement { center: [0.0, 0.0, -1.0], right: [0.0; 3], up: [0.0; 3] };
        let (Some((x, y)), Some(backdrop)) = (pointer, self.marker_backdrop) else {
            return hidden;
        };
        let s = self.marker_source;
        if !(x >= s.x && x <= s.x + s.w && y >= s.y && y <= s.y + s.h) {
            return hidden;
        }
        let uv = |r: Rect| (((x - r.pos.x) / r.size.x.max(1.0)) as f32, ((y - r.pos.y) / r.size.y.max(1.0)) as f32);
        match self.marker_targets.iter().find(|(r, _)| r.contains(dvec2(x, y))) {
            Some((r, placed)) => {
                let (u, v) = uv(*r);
                pointer_marker(placed, u, v)
            }
            None => {
                let r = Rect { pos: dvec2(s.x, s.y), size: dvec2(s.w, s.h) };
                let (u, v) = uv(r);
                pointer_marker(&backdrop, u, v)
            }
        }
    }

    fn set_quad(draw: &mut DrawStereoWindow, placement: &Placement) {
        draw.center = vec4(placement.center[0], placement.center[1], placement.center[2], 0.0);
        draw.axis_r = vec4(placement.right[0], placement.right[1], placement.right[2], 0.0);
        draw.axis_u = vec4(placement.up[0], placement.up[1], placement.up[2], 0.0);
    }

    /// Follow the tracker: every frame while it tracks (a slow poll while
    /// it does not), push the new head rotation into the retained draw
    /// calls and request a complete redraw.
    /// True when the desk must redraw: a headset's optics arrived or went
    /// away.
    pub fn handle_event(&mut self, cx: &mut Cx, event: &Event, look: (f32, f32), recenter: u32, headset: Option<&str>) -> bool {
        let tick = self.next.is_event(event).is_some();
        let polled = self.poll.is_event(event).is_some();
        if !tick && !polled {
            return false;
        }
        let (optics_changed, _) = self.tracker.display_calibration();
        if self.refresh_pose(cx, look, recenter, headset) {
            let values = self.head_uniforms();
            for area in &self.areas {
                let Some(inst) = area.valid_instance(cx).copied() else {
                    continue;
                };
                for (id, value) in &values {
                    cx.set_draw_item_uniform(inst.draw_list_id, inst.draw_item_id, *id, value);
                }
            }
        }
        if self.pose.tracking != Tracking::None {
            cx.redraw_all();
            self.next = cx.new_next_frame();
        } else {
            self.poll = cx.start_timeout(TRACKER_POLL);
        }
        optics_changed
    }
}

#[cfg(target_os = "linux")]
fn predicted_display_time(cx: &Cx, headset: Option<&str>, now: f64, panel_delay: Option<f64>) -> f64 {
    let Some(timing) = headset.and_then(|name| cx.linux_display_frame_timing(name)) else {
        return now;
    };
    // A display's scanout estimate and its own illumination delay, not a
    // headset-specific prediction constant. Preview stays unpredicted.
    let target = timing.present_time + panel_delay.unwrap_or(0.0);
    makepad_widgets::makepad_platform::trace!("wm.vr", "display lead_ms={:.3} refresh_ms={:.3} panel_ms={:.3}",
        (target - now) * 1000.0, timing.refresh_interval * 1000.0, panel_delay.unwrap_or(0.0) * 1000.0);
    target.max(now)
}

#[cfg(not(target_os = "linux"))]
fn predicted_display_time(_cx: &Cx, _headset: Option<&str>, now: f64, _panel_delay: Option<f64>) -> f64 {
    now
}
