//! House Art-Net / DMX primitives for the room lighting desk.
//!
//! The fixture patch was originally embedded in `makepad-example-automate`.
//! Keeping it here gives the automation dashboard and VJ console one Art-Net
//! packet implementation and one authoritative map of the room.
//!
//! VJ owns the APC40 clip grid and crossfader. The rest of that controller
//! is the automate lighting desk (`desk`), which writes the full room patch
//! through [`apply_dmx_mapping`]. The video-reactive renderer is a narrower
//! overlay and must not steal gobo / laser / smoke channels on its own.

mod desk;

pub use desk::{
    is_vj_reserved_midi, DeskEvent, DeskState, PresetBank, RoomShow, RoomSnapshot, NOTE_POWER,
    NOTE_WRITE, SCENE_COUNT,
};

use makepad_micro_serde::*;
use makepad_platform::Cx;
use std::{
    net::UdpSocket,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
#[cfg(not(target_arch = "wasm32"))]
use std::thread::JoinHandle;

pub const DMX_FRAME_HZ: f64 = 44.0;
pub const DMX_FRAME_DT: f64 = 1.0 / DMX_FRAME_HZ;
pub const ARTNET_BIND_ADDR: &str = "0.0.0.0:0";
pub const ARTNET_BROADCAST_ADDR: &str = "255.255.255.255:6454";
pub const CONTROLLER_INSTANCE_LOCK_ADDR: &str = "127.0.0.1:64640";
pub const DMX_LEN: usize = 512;
pub const ARTNET_HEADER_LEN: usize = 18;
pub const ARTNET_PACKET_LEN: usize = ARTNET_HEADER_LEN + DMX_LEN;

/// One authoritative universe-zero fixture patch. Addresses are one-based,
/// matching the labels on the room fixtures and their manuals.
pub mod fixture_patch {
    pub const RGB_LASER: usize = 110;
    pub const RGB_STROBE: usize = 120;
    pub const MOVING_WASHES: [usize; 2] = [200, 250];
    pub const SMOKE: [usize; 2] = [300, 310];
    pub const BEAM_LASERS: [usize; 5] = [400, 420, 440, 460, 480];
    pub const UV: [usize; 3] = [500, 502, 504];
}

pub const DMXOUTPUT_HEADER: [u8; ARTNET_HEADER_LEN] = [
    b'A', b'r', b't', b'-', b'N', b'e', b't', b'\0', 0x00, 0x50, 0x00, 0x0e, 0x00, 0x00, 0x00,
    0x00, 0x02, 0x00,
];

/// Complete ArtDmx packet for universe zero.
pub struct ArtNetPacket {
    bytes: [u8; ARTNET_PACKET_LEN],
}

impl Default for ArtNetPacket {
    fn default() -> Self {
        let mut bytes = [0; ARTNET_PACKET_LEN];
        bytes[..ARTNET_HEADER_LEN].copy_from_slice(&DMXOUTPUT_HEADER);
        Self { bytes }
    }
}

impl ArtNetPacket {
    pub fn dmx_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[ARTNET_HEADER_LEN..]
    }

    pub fn set_sequence(&mut self, sequence: u8) {
        // Art-Net sequence zero disables sequencing, so cycle through 1..255.
        self.bytes[12] = sequence.max(1);
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Clone, Copy, Default, SerRon, DeRon)]
pub struct ControllerState {
    pub fade: [f32; 9],
    pub tempo: f32,
    pub dial_0: [f32; 8],
    pub dial_1: [f32; 8],
    pub dial_2: [f32; 8],
    pub dial_3: [f32; 8],
    pub dial_4: [f32; 8],
    pub dial_5: [f32; 8],
    pub dial_6: [f32; 8],
    pub dial_7: [f32; 8],
    pub dial_top: [f32; 8],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ControllerButtons {
    pub preset: [bool; 13],
    pub write_preset: bool,
    pub power: bool,
}

pub fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn hsv_to_rgb(mut hue: f32, sat: f32, val: f32) -> (f32, f32, f32) {
    hue = clamp01(hue) * 6.0;
    let sector = hue.floor() as i32;
    let fract = hue - sector as f32;
    let p = val * (1.0 - sat);
    let q = val * (1.0 - sat * fract);
    let t = val * (1.0 - sat * (1.0 - fract));
    match sector {
        0 => (val, t, p),
        1 => (q, val, p),
        2 => (p, val, t),
        3 => (p, q, val),
        4 => (t, p, val),
        _ => (val, p, q),
    }
}

fn map_wargb(value: f32, fade: f32, out: &mut [u8], bases: &[usize]) {
    let fade = clamp01(fade);
    let (r, g, b) = hsv_to_rgb(value, 1.0, fade);
    map_rgb([r, g, b], 1.0, out, bases);
}

fn map_rgb(rgb: [f32; 3], gain: f32, out: &mut [u8], bases: &[usize]) {
    for base in bases {
        if *base == 0 || *base + 1 >= out.len() {
            continue;
        }
        // Keep the automation dashboard's established float-to-DMX
        // quantisation exactly: extraction into this crate must not move an
        // existing fixture by a code point.
        out[*base - 1] = (clamp01(rgb[0] * gain) * 255.0) as u8;
        out[*base] = (clamp01(rgb[1] * gain) * 255.0) as u8;
        out[*base + 1] = (clamp01(rgb[2] * gain) * 255.0) as u8;
    }
}

fn dmx_u8(value: u8, out: &mut [u8], bases: &[usize], channel: usize) {
    for base in bases {
        let index = base.saturating_sub(1) + channel.saturating_sub(1);
        if index < out.len() {
            out[index] = value;
        }
    }
}

fn dmx_f32(value: f32, out: &mut [u8], bases: &[usize], channel: usize) {
    dmx_u8((clamp01(value) * 255.0) as u8, out, bases, channel);
}

/// Render the full room patch used by the automation dashboard.
pub fn apply_dmx_mapping(
    state: &ControllerState,
    buttons: &ControllerButtons,
    dmx: &mut [u8],
    clock: f64,
) {
    if !buttons.power {
        return;
    }

    map_wargb(state.dial_top[3], 1.0, dmx, &[fixture_patch::RGB_LASER + 1]);
    let rgb_laser_addr = fixture_patch::RGB_LASER;
    match (state.fade[3] * 3.0) as usize {
        0 => dmx_u8(0, dmx, &[rgb_laser_addr], 1),
        1 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(0.75, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        2 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(1.0, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        _ => {}
    }
    map_wargb(state.dial_top[3], 1.0, dmx, &[rgb_laser_addr + 1]);
    match (state.fade[3] * 4.0) as usize {
        0 | 3 => dmx_u8(0, dmx, &[rgb_laser_addr], 1),
        1 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(1.0, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        2 => {
            dmx_u8(255, dmx, &[rgb_laser_addr], 1);
            dmx_f32(0.75, dmx, &[rgb_laser_addr], 6);
            dmx_u8(32, dmx, &[rgb_laser_addr], 7);
        }
        _ => {}
    }

    let rgb_strobe = fixture_patch::RGB_STROBE;
    map_wargb(state.dial_top[3], state.fade[3], dmx, &[rgb_strobe + 2]);
    dmx_f32(state.fade[3], dmx, &[rgb_strobe], 1);
    dmx_f32(state.tempo, dmx, &[rgb_strobe], 10);
    dmx_f32(state.dial_3[0], dmx, &[rgb_strobe], 13);
    dmx_f32(state.dial_3[1], dmx, &[rgb_strobe], 14);
    dmx_f32(state.dial_3[2], dmx, &[rgb_strobe], 15);
    dmx_f32(state.dial_3[3], dmx, &[rgb_strobe], 16);
    dmx_f32(state.dial_3[4], dmx, &[rgb_strobe], 17);
    dmx_f32(state.fade[4], dmx, &[rgb_strobe], 6);
    dmx_f32(state.tempo, dmx, &[rgb_strobe], 8);
    dmx_f32(state.dial_4[0], dmx, &[rgb_strobe], 11);
    dmx_f32(state.dial_4[1], dmx, &[rgb_strobe], 12);

    let [spot1, spot2] = fixture_patch::MOVING_WASHES;
    dmx_f32(state.fade[1], dmx, &[spot1, spot2], 6);
    dmx_f32(state.dial_1[0], dmx, &[spot1, spot2], 1);
    dmx_f32(state.dial_1[1], dmx, &[spot1, spot2], 3);
    dmx_f32(state.dial_top[1], dmx, &[spot1, spot2], 8);
    dmx_f32(state.dial_1[4], dmx, &[spot1, spot2], 12);
    dmx_f32(state.dial_1[3], dmx, &[spot1, spot2], 13);
    dmx_f32(state.dial_1[2], dmx, &[spot1, spot2], 10);
    dmx_f32(state.fade[2], dmx, &[spot1, spot2], 14);
    map_wargb(state.dial_top[2], 1.0, dmx, &[spot1 + 15, spot2 + 15]);

    let [smoke, smoke_2] = fixture_patch::SMOKE;
    let slot_len = 101.0f64;
    let needed = slot_len * state.dial_0[0] as f64;
    let t = clock.rem_euclid(slot_len);
    dmx_f32(if t < needed { 1.0 } else { 0.0 }, dmx, &[smoke], 1);
    dmx_f32(state.dial_0[2], dmx, &[smoke_2], 1);
    dmx_f32(state.dial_0[1], dmx, &[smoke_2], 2);

    let lasers = fixture_patch::BEAM_LASERS;
    dmx_f32(state.fade[5], dmx, &lasers, 1);
    dmx_f32(state.dial_5[0], dmx, &lasers, 2);
    dmx_f32(state.dial_top[5], dmx, &lasers, 11);
    dmx_f32(state.dial_5[1], dmx, &lasers, 12);
    dmx_f32(0.5, dmx, &lasers, 3);
    dmx_f32(0.3, dmx, &lasers, 4);
    dmx_f32(state.dial_5[2], dmx, &lasers, 5);
    dmx_f32(state.dial_5[3], dmx, &lasers, 6);
    dmx_f32(0.5, dmx, &lasers, 7);
    dmx_f32(0.5, dmx, &lasers, 8);
    dmx_f32(0.5, dmx, &lasers, 9);
    dmx_f32(0.5, dmx, &lasers, 10);

    let uv = fixture_patch::UV;
    dmx_f32(state.fade[6], dmx, &uv, 1);
    dmx_f32(if state.tempo < 0.1 { 0.0 } else { state.tempo }, dmx, &uv, 2);
    dmx_f32(if state.fade[7] > 0.5 { 1.0 } else { 0.0 }, dmx, &uv, 3);
}

/// Color and perceptual intensity for one part of a frame. `rgb` describes
/// chroma (normally peak-normalized); `intensity` carries brightness so a
/// renderer does not accidentally square the scene luminance.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LightSample {
    pub rgb: [f32; 3],
    pub intensity: f32,
}

impl LightSample {
    pub fn blend(a: Option<Self>, b: Option<Self>, mix: f32) -> Self {
        let a = a.unwrap_or_default();
        let b = b.unwrap_or_default();
        let t = clamp01(mix);
        Self {
            rgb: [
                a.rgb[0] + (b.rgb[0] - a.rgb[0]) * t,
                a.rgb[1] + (b.rgb[1] - a.rgb[1]) * t,
                a.rgb[2] + (b.rgb[2] - a.rgb[2]) * t,
            ],
            intensity: a.intensity + (b.intensity - a.intensity) * t,
        }
    }
}

pub const LIGHT_ZONE_COUNT: usize = 5;

/// Stable ordering for the five ambilight sampling regions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum LightZone {
    Left = 0,
    Right = 1,
    Top = 2,
    Bottom = 3,
    Center = 4,
}

impl LightZone {
    pub const ALL: [Self; LIGHT_ZONE_COUNT] = [
        Self::Left,
        Self::Right,
        Self::Top,
        Self::Bottom,
        Self::Center,
    ];
}

/// A global sample plus edge-weighted samples used to place color around the
/// room. Keeping [`LightSample`] unchanged preserves simple/legacy callers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpatialLightSample {
    pub overall: LightSample,
    pub zones: [LightSample; LIGHT_ZONE_COUNT],
}

impl SpatialLightSample {
    pub fn uniform(sample: LightSample) -> Self {
        Self { overall: sample, zones: [sample; LIGHT_ZONE_COUNT] }
    }

    pub fn zone(&self, zone: LightZone) -> LightSample {
        self.zones[zone as usize]
    }

    pub fn blend(a: Option<Self>, b: Option<Self>, mix: f32) -> Self {
        let a = a.unwrap_or_default();
        let b = b.unwrap_or_default();
        let mut zones = [LightSample::default(); LIGHT_ZONE_COUNT];
        for zone in LightZone::ALL {
            zones[zone as usize] = LightSample::blend(
                Some(a.zones[zone as usize]),
                Some(b.zones[zone as usize]),
                mix,
            );
        }
        Self {
            overall: LightSample::blend(Some(a.overall), Some(b.overall), mix),
            zones,
        }
    }
}

impl From<LightSample> for SpatialLightSample {
    fn from(sample: LightSample) -> Self {
        Self::uniform(sample)
    }
}

/// Bounded, allocation-free analyzer tuning. Temporal values are per
/// presented frame and intentionally explicit so a UI can adjust response
/// without restarting the Art-Net worker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VideoLightConfig {
    pub max_samples: usize,
    pub edge_fraction: f32,
    pub black_floor: f32,
    pub exposure_target: f32,
    pub max_exposure: f32,
    pub max_intensity: f32,
    pub saturation_boost: f32,
    pub attack: f32,
    pub decay: f32,
    pub scene_cut_threshold: f32,
    pub scene_cut_attack: f32,
}

impl Default for VideoLightConfig {
    fn default() -> Self {
        Self {
            max_samples: 4096,
            edge_fraction: 0.30,
            black_floor: 0.012,
            exposure_target: 0.50,
            max_exposure: 3.0,
            max_intensity: 0.72,
            saturation_boost: 1.25,
            attack: 0.46,
            decay: 0.16,
            scene_cut_threshold: 0.30,
            scene_cut_attack: 0.88,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct HueBin {
    weight: f64,
    rgb: [f64; 3],
}

#[derive(Clone, Copy, Debug, Default)]
struct ZoneAccumulator {
    spatial_weight: f64,
    luma_sq: f64,
    signal_weight: f64,
    hue_bins: [HueBin; 12],
}

impl ZoneAccumulator {
    fn add(&mut self, rgb: [f32; 3], luma: f32, weight: f32, black_floor: f32) {
        if weight <= 0.0 {
            return;
        }
        let weight = weight as f64;
        self.spatial_weight += weight;
        let gate_width = 0.035;
        let gate = smooth01((luma - black_floor) / gate_width);
        let corrected = clamp01((luma - black_floor) / (1.0 - black_floor).max(0.01)) * gate;
        self.luma_sq += (corrected * corrected) as f64 * weight;
        self.signal_weight += corrected as f64 * weight;

        let peak = rgb[0].max(rgb[1]).max(rgb[2]);
        let low = rgb[0].min(rgb[1]).min(rgb[2]);
        if corrected <= 0.0 || peak <= 0.0 {
            return;
        }
        let saturation = (peak - low) / peak;
        if saturation < 0.04 {
            return;
        }
        let hue = rgb_hue(rgb, peak, low);
        let bin = ((hue * 12.0).floor() as usize).min(11);
        // Saturated mid-tones should beat a large neutral background, while
        // clipped white highlights and barely-colored compression noise do
        // not get to choose the room hue.
        let highlight = smooth01((luma - 0.78) / 0.22);
        let color_weight = weight
            * corrected.sqrt() as f64
            * (saturation * saturation) as f64
            * (1.0 - 0.72 * highlight as f64);
        let normalized = [rgb[0] / peak, rgb[1] / peak, rgb[2] / peak];
        self.hue_bins[bin].weight += color_weight;
        for (out, value) in self.hue_bins[bin].rgb.iter_mut().zip(normalized) {
            *out += value as f64 * color_weight;
        }
    }

    fn raw_intensity(&self) -> f32 {
        if self.spatial_weight <= f64::EPSILON {
            0.0
        } else {
            (self.luma_sq / self.spatial_weight).sqrt() as f32
        }
    }

    fn finish(&self, exposure: f32, config: VideoLightConfig) -> LightSample {
        let intensity = (self.raw_intensity() * exposure).min(config.max_intensity);
        if intensity <= 0.0005 {
            return LightSample::default();
        }
        let Some((strongest, strongest_bin)) = self
            .hue_bins
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.weight.total_cmp(&b.1.weight))
        else {
            return LightSample { rgb: [1.0; 3], intensity };
        };
        if strongest_bin.weight <= self.signal_weight * 0.001 {
            return LightSample { rgb: [1.0; 3], intensity };
        }

        // Include neighboring hue bins at half weight to make bin boundaries
        // continuous without allocating or sorting a histogram.
        let neighbors = [(strongest + 11) % 12, strongest, (strongest + 1) % 12];
        let factors = [0.5, 1.0, 0.5];
        let mut rgb = [0.0f64; 3];
        let mut color_weight = 0.0f64;
        for (bin, factor) in neighbors.into_iter().zip(factors) {
            let contribution = self.hue_bins[bin].weight * factor;
            color_weight += contribution;
            if self.hue_bins[bin].weight > f64::EPSILON {
                for (channel, sum) in rgb.iter_mut().zip(self.hue_bins[bin].rgb) {
                    *channel += sum / self.hue_bins[bin].weight * contribution;
                }
            }
        }
        if color_weight <= f64::EPSILON {
            return LightSample { rgb: [1.0; 3], intensity };
        }
        let mut rgb = rgb.map(|channel| (channel / color_weight) as f32);
        boost_saturation(&mut rgb, config.saturation_boost);
        LightSample { rgb, intensity }
    }
}

fn smooth01(value: f32) -> f32 {
    let value = clamp01(value);
    value * value * (3.0 - 2.0 * value)
}

fn rgb_hue(rgb: [f32; 3], peak: f32, low: f32) -> f32 {
    let chroma = peak - low;
    if chroma <= f32::EPSILON {
        return 0.0;
    }
    let hue = if peak == rgb[0] {
        ((rgb[1] - rgb[2]) / chroma).rem_euclid(6.0)
    } else if peak == rgb[1] {
        (rgb[2] - rgb[0]) / chroma + 2.0
    } else {
        (rgb[0] - rgb[1]) / chroma + 4.0
    };
    hue / 6.0
}

fn boost_saturation(rgb: &mut [f32; 3], amount: f32) {
    let peak = rgb[0].max(rgb[1]).max(rgb[2]);
    let low = rgb[0].min(rgb[1]).min(rgb[2]);
    let chroma = peak - low;
    if peak <= f32::EPSILON || chroma <= f32::EPSILON {
        *rgb = [1.0; 3];
        return;
    }
    for channel in rgb.iter_mut() {
        *channel /= peak;
    }
    let low = low / peak;
    let saturation = 1.0 - low;
    let target = clamp01(saturation * amount.max(0.0));
    for channel in rgb.iter_mut() {
        let hue_channel = (*channel - low) / saturation.max(f32::EPSILON);
        *channel = 1.0 - target + hue_channel * target;
    }
}

pub struct VideoLightAnalyzer {
    config: VideoLightConfig,
    current: Option<SpatialLightSample>,
    srgb_linear: [f32; 256],
}

impl Default for VideoLightAnalyzer {
    fn default() -> Self {
        Self::new(VideoLightConfig::default())
    }
}

impl VideoLightAnalyzer {
    pub fn new(config: VideoLightConfig) -> Self {
        let srgb_linear = std::array::from_fn(|value| {
            let value = value as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        });
        Self { config, current: None, srgb_linear }
    }

    pub fn config(&self) -> VideoLightConfig {
        self.config
    }

    pub fn set_config(&mut self, config: VideoLightConfig) {
        self.config = config;
    }

    pub fn set_black_floor(&mut self, black_floor: f32) {
        self.config.black_floor = black_floor;
    }

    pub fn set_saturation_boost(&mut self, saturation_boost: f32) {
        self.config.saturation_boost = saturation_boost;
    }

    /// Set a single live response control: zero is a slow, lingering fade and
    /// one follows cuts and brightness changes aggressively.
    pub fn set_response(&mut self, response: f32) {
        let response = clamp01(response);
        self.config.attack = 0.18 + 0.62 * response;
        self.config.decay = 0.035 + 0.30 * response;
        self.config.scene_cut_attack = 0.70 + 0.30 * response;
    }

    pub fn reset(&mut self) {
        self.current = None;
    }

    /// Compatibility path for callers that do not know frame geometry.
    pub fn push_bgra(&mut self, pixels: &[u32]) -> LightSample {
        self.push_bgra_frame(pixels, pixels.len(), 1).overall
    }

    /// Analyze a deterministic grid of at most `max_samples` BGRA pixels.
    /// Pixels are `u32::from_le_bytes([b, g, r, a])`; alpha is ignored.
    pub fn push_bgra_frame(
        &mut self,
        pixels: &[u32],
        width: usize,
        height: usize,
    ) -> SpatialLightSample {
        let Some(pixel_count) = width.checked_mul(height) else {
            return self.current.unwrap_or_default();
        };
        if width == 0 || height == 0 || pixels.len() < pixel_count {
            return self.current.unwrap_or_default();
        }
        let config = sanitized_video_config(self.config);
        let budget = config.max_samples.max(1).min(pixel_count);
        let aspect = width as f64 / height as f64;
        let columns = ((budget as f64 * aspect).sqrt().round() as usize)
            .max(1)
            .min(width)
            .min(budget);
        let rows = (budget / columns).max(1).min(height);
        let mut global = ZoneAccumulator::default();
        let mut zones = [ZoneAccumulator::default(); LIGHT_ZONE_COUNT];

        for row in 0..rows {
            let y = ((row * 2 + 1) * height / (rows * 2)).min(height - 1);
            let fy = (y as f32 + 0.5) / height as f32;
            for column in 0..columns {
                let x = ((column * 2 + 1) * width / (columns * 2)).min(width - 1);
                let fx = (x as f32 + 0.5) / width as f32;
                let [b, g, r, _] = pixels[y * width + x].to_le_bytes();
                let rgb = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0];
                let linear = [
                    self.srgb_linear[r as usize],
                    self.srgb_linear[g as usize],
                    self.srgb_linear[b as usize],
                ];
                let luma = (0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2])
                    .sqrt();
                global.add(rgb, luma, 1.0, config.black_floor);

                let edge = config.edge_fraction;
                let left = smooth01((edge - fx) / edge);
                let right = smooth01((edge - (1.0 - fx)) / edge);
                let top = smooth01((edge - fy) / edge);
                let bottom = smooth01((edge - (1.0 - fy)) / edge);
                let center = (1.0 - left.max(right)) * (1.0 - top.max(bottom));
                let weights = [left, right, top, bottom, center];
                for (zone, weight) in zones.iter_mut().zip(weights) {
                    zone.add(rgb, luma, weight, config.black_floor);
                }
            }
        }

        let reference = global.raw_intensity();
        let exposure = if reference <= f32::EPSILON {
            1.0
        } else {
            (config.exposure_target / reference).clamp(0.25, config.max_exposure)
        };
        let raw = SpatialLightSample {
            overall: global.finish(exposure, config),
            zones: zones.map(|zone| zone.finish(exposure, config)),
        };
        let next = self
            .current
            .map(|current| smooth_spatial_sample(current, raw, config))
            .unwrap_or(raw);
        self.current = Some(next);
        next
    }
}

fn sanitized_video_config(mut config: VideoLightConfig) -> VideoLightConfig {
    config.edge_fraction = config.edge_fraction.clamp(0.05, 0.49);
    config.black_floor = config.black_floor.clamp(0.0, 0.25);
    config.exposure_target = clamp01(config.exposure_target);
    config.max_exposure = config.max_exposure.clamp(0.25, 8.0);
    config.max_intensity = clamp01(config.max_intensity);
    config.saturation_boost = config.saturation_boost.clamp(0.0, 3.0);
    config.attack = clamp01(config.attack);
    config.decay = clamp01(config.decay);
    config.scene_cut_threshold = clamp01(config.scene_cut_threshold);
    config.scene_cut_attack = clamp01(config.scene_cut_attack);
    config
}

fn smooth_spatial_sample(
    old: SpatialLightSample,
    raw: SpatialLightSample,
    config: VideoLightConfig,
) -> SpatialLightSample {
    let mut zones = [LightSample::default(); LIGHT_ZONE_COUNT];
    for zone in LightZone::ALL {
        zones[zone as usize] = smooth_light_sample(
            old.zones[zone as usize],
            raw.zones[zone as usize],
            config,
        );
    }
    SpatialLightSample {
        overall: smooth_light_sample(old.overall, raw.overall, config),
        zones,
    }
}

fn smooth_light_sample(
    old: LightSample,
    raw: LightSample,
    config: VideoLightConfig,
) -> LightSample {
    let color_distance = ((old.rgb[0] - raw.rgb[0]).abs()
        + (old.rgb[1] - raw.rgb[1]).abs()
        + (old.rgb[2] - raw.rgb[2]).abs())
        / 3.0;
    let intensity_distance = (old.intensity - raw.intensity).abs();
    let meaningful_color = old.intensity.min(raw.intensity) > 0.035;
    let is_cut = (meaningful_color && color_distance >= config.scene_cut_threshold)
        || (raw.intensity > old.intensity
            && intensity_distance >= config.scene_cut_threshold * 0.6);
    let alpha = if is_cut {
        config.scene_cut_attack
    } else if raw.intensity >= old.intensity {
        config.attack
    } else {
        config.decay
    };
    let mut next = LightSample {
        rgb: [
            old.rgb[0] + (raw.rgb[0] - old.rgb[0]) * alpha,
            old.rgb[1] + (raw.rgb[1] - old.rgb[1]) * alpha,
            old.rgb[2] + (raw.rgb[2] - old.rgb[2]) * alpha,
        ],
        intensity: old.intensity + (raw.intensity - old.intensity) * alpha,
    };
    if next.intensity <= 0.0005 {
        next = LightSample::default();
    }
    next
}

fn normalized_chroma(sample: LightSample) -> [f32; 3] {
    let peak = sample.rgb[0].max(sample.rgb[1]).max(sample.rgb[2]);
    if peak <= 0.001 {
        [0.0; 3]
    } else {
        [
            clamp01(sample.rgb[0] / peak),
            clamp01(sample.rgb[1] / peak),
            clamp01(sample.rgb[2] / peak),
        ]
    }
}

fn weighted_light<const N: usize>(samples: [(LightSample, f32); N]) -> LightSample {
    let weight_sum = samples.iter().map(|(_, weight)| weight.max(0.0)).sum::<f32>();
    if weight_sum <= f32::EPSILON {
        return LightSample::default();
    }
    let intensity = samples
        .iter()
        .map(|(sample, weight)| sample.intensity * weight.max(0.0))
        .sum::<f32>()
        / weight_sum;
    let color_weight = samples
        .iter()
        .map(|(sample, weight)| sample.intensity * weight.max(0.0))
        .sum::<f32>();
    if color_weight <= f32::EPSILON {
        return LightSample::default();
    }
    let mut rgb = [0.0; 3];
    for (sample, weight) in samples {
        let weight = sample.intensity * weight.max(0.0) / color_weight;
        for (output, channel) in rgb.iter_mut().zip(sample.rgb) {
            *output += channel * weight;
        }
    }
    LightSample { rgb, intensity }
}

fn park_movers(dmx: &mut [u8], positions: [[f32; 2]; 2]) {
    for (base, position) in fixture_patch::MOVING_WASHES.into_iter().zip(positions) {
        dmx_f32(position[0], dmx, &[base], 1);
        dmx_f32(position[1], dmx, &[base], 3);
    }
}

fn render_ambilight_layer(
    sample: SpatialLightSample,
    master: f32,
    mover_park: [[f32; 2]; 2],
    dmx: &mut [u8],
) {
    park_movers(dmx, mover_park);
    let fixtures = [
        weighted_light([
            (sample.zone(LightZone::Center), 0.55),
            (sample.zone(LightZone::Top), 0.25),
            (sample.zone(LightZone::Bottom), 0.20),
        ]),
        weighted_light([
            (sample.zone(LightZone::Left), 0.70),
            (sample.zone(LightZone::Top), 0.18),
            (sample.zone(LightZone::Bottom), 0.12),
        ]),
        weighted_light([
            (sample.zone(LightZone::Right), 0.70),
            (sample.zone(LightZone::Top), 0.18),
            (sample.zone(LightZone::Bottom), 0.12),
        ]),
    ];
    let master = clamp01(master);

    let rgb_strobe = fixtures[0];
    let level = clamp01(rgb_strobe.intensity * master);
    dmx_f32(level, dmx, &[fixture_patch::RGB_STROBE], 1);
    map_rgb(normalized_chroma(rgb_strobe), 1.0, dmx, &[fixture_patch::RGB_STROBE + 2]);

    for (base, fixture) in fixture_patch::MOVING_WASHES.into_iter().zip(&fixtures[1..]) {
        let level = clamp01(fixture.intensity * master);
        dmx_f32(level, dmx, &[base], 6);
        dmx_f32(level, dmx, &[base], 14);
        map_rgb(normalized_chroma(*fixture), 1.0, dmx, &[base + 15]);
    }
}

/// Render a safe spatial ambilight scene. The entire universe is cleared
/// first; smoke, lasers, UV, strobe/macro channels, and mover effects remain
/// unreachable from this API.
pub fn render_spatial_video_lights(
    sample: SpatialLightSample,
    master: f32,
    dmx: &mut [u8],
) {
    dmx.fill(0);
    render_ambilight_layer(sample, master, [[0.5, 0.5]; 2], dmx);
}

/// Compatibility renderer for a single global color sample.
pub fn render_video_lights(sample: LightSample, master: f32, dmx: &mut [u8]) {
    render_spatial_video_lights(sample.into(), master, dmx);
}

/// Explicit color/dimmer command. This contains no arming state and therefore
/// cannot grant access to hazardous fixtures by itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorControl {
    pub rgb: [f32; 3],
    pub level: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoverControl {
    pub position: [f32; 2],
    pub rgb: [f32; 3],
    pub level: f32,
}

impl Default for MoverControl {
    fn default() -> Self {
        Self { position: [0.5, 0.5], rgb: [1.0; 3], level: 0.0 }
    }
}

/// Manual strobe request for the RGB/strobe fixture. Video analysis never
/// constructs this type or writes these channels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StrobeControl {
    pub level: f32,
    pub rate: f32,
}

/// Desired hazardous outputs. Actual output additionally requires a separately
/// armed group and a live press-and-hold heartbeat.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HazardControl {
    pub rgb_laser: ColorControl,
    pub beam_lasers: ColorControl,
    pub beam_pattern: f32,
    pub smoke: [f32; 2],
    pub uv: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HazardArms {
    pub lasers: bool,
    pub smoke: bool,
    pub uv: bool,
}

impl HazardArms {
    pub fn any(self) -> bool {
        self.lasers || self.smoke || self.uv
    }
}

/// Runtime controls are a small `Copy` snapshot. The latest spatial video
/// sample and the safety interlock live separately, preventing an automatic
/// source from accidentally setting an arm bit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PerformanceState {
    pub master: f32,
    pub blackout: bool,
    pub ambilight_level: f32,
    pub rgb: ColorControl,
    pub strobe: StrobeControl,
    pub movers: [MoverControl; 2],
    pub hazards: HazardControl,
}

impl Default for PerformanceState {
    fn default() -> Self {
        Self {
            master: 0.35,
            blackout: false,
            ambilight_level: 1.0,
            rgb: ColorControl::default(),
            strobe: StrobeControl::default(),
            movers: [MoverControl::default(); 2],
            hazards: HazardControl::default(),
        }
    }
}

/// Hard limits applied after UI/manual values. These are startup policy, not
/// performance controls, so a cue cannot raise its own safety ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerCaps {
    pub ambilight: f32,
    pub rgb: f32,
    pub movers: f32,
    pub strobe: f32,
    pub lasers: f32,
    pub smoke: f32,
    pub uv: f32,
}

impl Default for PowerCaps {
    fn default() -> Self {
        Self {
            ambilight: 1.0,
            rgb: 0.75,
            movers: 0.75,
            strobe: 0.20,
            lasers: 0.20,
            smoke: 0.50,
            uv: 0.50,
        }
    }
}

/// Startup policy for full-room control. `control_timeout` fails the complete
/// universe dark if the owner stops updating; hazards have the shorter
/// `hazard_heartbeat_timeout` in addition to that watchdog.
#[derive(Clone, Debug)]
pub struct PerformanceConfig {
    pub artnet: ArtNetConfig,
    pub power_caps: PowerCaps,
    pub mover_park: [[f32; 2]; 2],
    pub hazard_heartbeat_timeout: Duration,
    pub control_timeout: Duration,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            artnet: ArtNetConfig::default(),
            power_caps: PowerCaps::default(),
            mover_park: [[0.5, 0.5]; 2],
            hazard_heartbeat_timeout: Duration::from_millis(250),
            control_timeout: Duration::from_secs(2),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SafetySnapshot {
    control_live: bool,
    deadman_live: bool,
    arms: HazardArms,
}

fn rgb_to_hue(rgb: [f32; 3]) -> f32 {
    let rgb = rgb.map(clamp01);
    let peak = rgb[0].max(rgb[1]).max(rgb[2]);
    let low = rgb[0].min(rgb[1]).min(rgb[2]);
    rgb_hue(rgb, peak, low)
}

fn render_terminal_blackout(dmx: &mut [u8], mover_park: [[f32; 2]; 2]) {
    dmx.fill(0);
    park_movers(dmx, mover_park);
}

fn render_performance_lights(
    sample: SpatialLightSample,
    state: PerformanceState,
    safety: SafetySnapshot,
    output_master: f32,
    caps: PowerCaps,
    mover_park: [[f32; 2]; 2],
    dmx: &mut [u8],
) {
    if !safety.control_live || state.blackout {
        render_terminal_blackout(dmx, mover_park);
        return;
    }
    dmx.fill(0);
    let master = clamp01(state.master) * clamp01(output_master);
    render_ambilight_layer(
        sample,
        master * clamp01(state.ambilight_level) * clamp01(caps.ambilight),
        mover_park,
        dmx,
    );

    // Explicit manual RGB overrides the automatic color only while it has a
    // nonzero level. Strobe channels remain a wholly manual layer.
    let rgb_level = clamp01(state.rgb.level) * clamp01(caps.rgb) * master;
    if rgb_level > 0.0 {
        dmx_f32(rgb_level, dmx, &[fixture_patch::RGB_STROBE], 1);
        map_rgb(
            state.rgb.rgb.map(clamp01),
            1.0,
            dmx,
            &[fixture_patch::RGB_STROBE + 2],
        );
    }
    let strobe_level = clamp01(state.strobe.level) * clamp01(caps.strobe) * master;
    if strobe_level > 0.0 {
        dmx_f32(strobe_level, dmx, &[fixture_patch::RGB_STROBE], 6);
        dmx_f32(state.strobe.rate, dmx, &[fixture_patch::RGB_STROBE], 8);
        dmx_f32(state.strobe.rate, dmx, &[fixture_patch::RGB_STROBE], 10);
    }

    for ((base, mover), park) in fixture_patch::MOVING_WASHES
        .into_iter()
        .zip(state.movers)
        .zip(mover_park)
    {
        let level = clamp01(mover.level) * clamp01(caps.movers) * master;
        if level <= 0.0 {
            continue;
        }
        dmx_f32(if mover.position[0].is_finite() { mover.position[0] } else { park[0] }, dmx, &[base], 1);
        dmx_f32(if mover.position[1].is_finite() { mover.position[1] } else { park[1] }, dmx, &[base], 3);
        dmx_f32(level, dmx, &[base], 6);
        dmx_f32(level, dmx, &[base], 14);
        map_rgb(mover.rgb.map(clamp01), 1.0, dmx, &[base + 15]);
    }

    if !safety.deadman_live {
        return;
    }
    if safety.arms.lasers {
        let laser_cap = clamp01(caps.lasers) * master;
        let rgb_level = clamp01(state.hazards.rgb_laser.level) * laser_cap;
        if rgb_level > 0.0 {
            dmx_u8(255, dmx, &[fixture_patch::RGB_LASER], 1);
            map_rgb(
                state.hazards.rgb_laser.rgb.map(clamp01),
                rgb_level,
                dmx,
                &[fixture_patch::RGB_LASER + 1],
            );
            dmx_f32(rgb_level, dmx, &[fixture_patch::RGB_LASER], 6);
        }
        let beam_level = clamp01(state.hazards.beam_lasers.level) * laser_cap;
        if beam_level > 0.0 {
            dmx_f32(beam_level, dmx, &fixture_patch::BEAM_LASERS, 1);
            dmx_f32(state.hazards.beam_pattern, dmx, &fixture_patch::BEAM_LASERS, 2);
            dmx_f32(
                rgb_to_hue(state.hazards.beam_lasers.rgb),
                dmx,
                &fixture_patch::BEAM_LASERS,
                11,
            );
        }
    }
    if safety.arms.smoke {
        let smoke_cap = clamp01(caps.smoke) * master;
        dmx_f32(state.hazards.smoke[0] * smoke_cap, dmx, &[fixture_patch::SMOKE[0]], 1);
        dmx_f32(state.hazards.smoke[1] * smoke_cap, dmx, &[fixture_patch::SMOKE[1]], 1);
        dmx_f32(state.hazards.smoke[1] * smoke_cap, dmx, &[fixture_patch::SMOKE[1]], 2);
    }
    if safety.arms.uv {
        dmx_f32(
            state.hazards.uv * clamp01(caps.uv) * master,
            dmx,
            &fixture_patch::UV,
            1,
        );
    }
}

#[derive(Clone, Debug)]
pub struct ArtNetConfig {
    pub bind_addr: String,
    pub target_addr: String,
    pub lock_addr: String,
    pub master: f32,
}

impl Default for ArtNetConfig {
    fn default() -> Self {
        Self {
            bind_addr: ARTNET_BIND_ADDR.to_string(),
            target_addr: ARTNET_BROADCAST_ADDR.to_string(),
            lock_addr: CONTROLLER_INSTANCE_LOCK_ADDR.to_string(),
            master: 0.85,
        }
    }
}

/// Latest-state, bounded Art-Net transmitter. Updates overwrite one shared
/// sample rather than queueing frames, so a stalled NIC cannot back up UI work.
pub struct VideoArtNet {
    shared: Arc<Mutex<PerformanceWorkerState>>,
    stop: Arc<AtomicBool>,
    #[cfg(not(target_arch = "wasm32"))]
    thread: Option<JoinHandle<()>>,
}

#[derive(Clone, Copy, Debug)]
struct PerformanceWorkerState {
    sample: SpatialLightSample,
    performance: PerformanceState,
    arms: HazardArms,
    deadman: bool,
    blackout_latched: bool,
    last_hazard_heartbeat: f64,
    last_control_update: f64,
}

fn safety_snapshot_at(
    state: PerformanceWorkerState,
    now: f64,
    hazard_timeout: Duration,
    control_timeout: Duration,
) -> SafetySnapshot {
    let control_live = control_timeout.is_zero()
        || now - state.last_control_update <= control_timeout.as_secs_f64();
    let deadman_live = state.deadman
        && !state.blackout_latched
        && (hazard_timeout.is_zero()
            || now - state.last_hazard_heartbeat <= hazard_timeout.as_secs_f64());
    SafetySnapshot { control_live, deadman_live, arms: state.arms }
}

impl VideoArtNet {
    /// Compatibility startup for ambilight-only callers.
    pub fn start(config: ArtNetConfig) -> Result<Self, String> {
        let mut performance = PerformanceState::default();
        performance.master = 1.0;
        let full_config = PerformanceConfig {
            artnet: config,
            ..PerformanceConfig::default()
        };
        Self::start_inner(full_config, performance)
    }

    pub fn start_performance(config: PerformanceConfig) -> Result<Self, String> {
        Self::start_inner(config, PerformanceState::default())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start_inner(
        config: PerformanceConfig,
        initial_performance: PerformanceState,
    ) -> Result<Self, String> {
        let instance_lock = UdpSocket::bind(&config.artnet.lock_addr).map_err(|e| {
            format!(
                "show control already active at {}: {e}",
                config.artnet.lock_addr
            )
        })?;
        let socket = UdpSocket::bind(&config.artnet.bind_addr).map_err(|e| {
            format!("Art-Net bind {} failed: {e}", config.artnet.bind_addr)
        })?;
        socket
            .set_broadcast(true)
            .map_err(|e| format!("Art-Net broadcast setup failed: {e}"))?;
        let now = Cx::monotonic_now();
        let shared = Arc::new(Mutex::new(PerformanceWorkerState {
            sample: SpatialLightSample::default(),
            performance: initial_performance,
            arms: HazardArms::default(),
            deadman: false,
            blackout_latched: false,
            last_hazard_heartbeat: now,
            last_control_update: now,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_shared = shared.clone();
        let worker_stop = stop.clone();
        let target = config.artnet.target_addr;
        let output_master = config.artnet.master;
        let caps = config.power_caps;
        let mover_park = config.mover_park;
        let hazard_timeout = config.hazard_heartbeat_timeout;
        let control_timeout = config.control_timeout;
        let thread = std::thread::Builder::new()
            .name("makepad-artnet-show-control".to_string())
            .spawn(move || {
                let _instance_lock = instance_lock;
                let mut packet = ArtNetPacket::default();
                let mut sequence = 1u8;
                let frame = Duration::from_secs_f64(DMX_FRAME_DT);
                while !worker_stop.load(Ordering::Acquire) {
                    let started = Cx::monotonic_now();
                    let snapshot = *worker_shared.lock().unwrap();
                    let safety =
                        safety_snapshot_at(snapshot, started, hazard_timeout, control_timeout);
                    let mut performance = snapshot.performance;
                    performance.blackout |= snapshot.blackout_latched;
                    render_performance_lights(
                        snapshot.sample,
                        performance,
                        safety,
                        output_master,
                        caps,
                        mover_park,
                        packet.dmx_mut(),
                    );
                    packet.set_sequence(sequence);
                    let _ = socket.send_to(packet.as_bytes(), &target);
                    sequence = if sequence == u8::MAX { 1 } else { sequence + 1 };
                    let spent = Duration::from_secs_f64(Cx::monotonic_now() - started);
                    if spent < frame {
                        std::thread::sleep(frame - spent);
                    }
                }
                // Art-Net receivers commonly hold the last universe. Always
                // hand them an explicit blackout before releasing the
                // single-controller lock, otherwise quitting the VJ can
                // strand fixtures at the last sampled frame.
                render_terminal_blackout(packet.dmx_mut(), mover_park);
                packet.set_sequence(sequence);
                let _ = socket.send_to(packet.as_bytes(), &target);
            })
            .map_err(|e| format!("Art-Net worker start failed: {e}"))?;
        Ok(Self { shared, stop, thread: Some(thread) })
    }

    #[cfg(target_arch = "wasm32")]
    fn start_inner(
        _config: PerformanceConfig,
        _initial_performance: PerformanceState,
    ) -> Result<Self, String> {
        Err("Art-Net output is unavailable on web".to_string())
    }

    pub fn set_sample(&self, sample: LightSample) {
        self.set_spatial_sample(sample.into());
    }

    pub fn set_spatial_sample(&self, sample: SpatialLightSample) {
        let mut shared = self.shared.lock().unwrap();
        shared.sample = sample;
        shared.last_control_update = Cx::monotonic_now();
    }

    pub fn set_performance_state(&self, mut state: PerformanceState) {
        let mut shared = self.shared.lock().unwrap();
        if state.blackout {
            shared.blackout_latched = true;
            shared.arms = HazardArms::default();
            shared.deadman = false;
        }
        state.blackout |= shared.blackout_latched;
        shared.performance = state;
        shared.last_control_update = Cx::monotonic_now();
    }

    pub fn set_master(&self, master: f32) {
        let mut shared = self.shared.lock().unwrap();
        shared.performance.master = master;
        shared.last_control_update = Cx::monotonic_now();
    }

    /// Arm selected groups, but do not energize them. Output additionally
    /// requires repeated `hazard_heartbeat(true)` calls inside the configured
    /// timeout window.
    pub fn set_hazard_arms(&self, arms: HazardArms) {
        let mut shared = self.shared.lock().unwrap();
        if !shared.blackout_latched {
            shared.arms = arms;
        }
        if !arms.any() {
            shared.deadman = false;
        }
        shared.last_control_update = Cx::monotonic_now();
    }

    pub fn hazard_heartbeat(&self, deadman_pressed: bool) {
        let now = Cx::monotonic_now();
        let mut shared = self.shared.lock().unwrap();
        shared.deadman = deadman_pressed && !shared.blackout_latched;
        shared.last_hazard_heartbeat = now;
        shared.last_control_update = now;
    }

    pub fn disarm_hazards(&self) {
        let mut shared = self.shared.lock().unwrap();
        shared.arms = HazardArms::default();
        shared.deadman = false;
        shared.last_control_update = Cx::monotonic_now();
    }

    /// Latch a whole-room blackout and synchronously revoke all hazard arms.
    pub fn blackout(&self) {
        let mut shared = self.shared.lock().unwrap();
        shared.blackout_latched = true;
        shared.performance.blackout = true;
        shared.arms = HazardArms::default();
        shared.deadman = false;
        shared.last_control_update = Cx::monotonic_now();
    }

    /// Clear only the blackout latch. Hazard groups remain disarmed and need
    /// an explicit new arm followed by a fresh deadman heartbeat.
    pub fn restore(&self) {
        let mut shared = self.shared.lock().unwrap();
        shared.blackout_latched = false;
        shared.performance.blackout = false;
        shared.arms = HazardArms::default();
        shared.deadman = false;
        shared.last_control_update = Cx::monotonic_now();
    }
}

/// Descriptive alias for new full-room callers; the legacy name remains valid.
pub type RoomArtNet = VideoArtNet;

impl Drop for VideoArtNet {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bgra(r: u8, g: u8, b: u8) -> u32 {
        u32::from_le_bytes([b, g, r, 255])
    }

    fn solid_frame(width: usize, height: usize, color: u32) -> Vec<u32> {
        vec![color; width * height]
    }

    fn assert_near(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "expected {expected} +/- {tolerance}, got {actual}"
        );
    }

    #[test]
    fn artnet_packet_is_valid_universe_zero_dmx512() {
        let mut packet = ArtNetPacket::default();
        packet.set_sequence(7);
        assert_eq!(&packet.as_bytes()[..8], b"Art-Net\0");
        assert_eq!(&packet.as_bytes()[8..12], &[0x00, 0x50, 0x00, 0x0e]);
        assert_eq!(packet.as_bytes()[12], 7);
        assert_eq!(&packet.as_bytes()[14..18], &[0, 0, 2, 0]);
        assert_eq!(packet.as_bytes().len(), 530);
    }

    #[test]
    fn video_renderer_cannot_touch_effect_fixture_ranges() {
        let mut dmx = [0xa5; DMX_LEN];
        render_video_lights(
            LightSample { rgb: [0.8, 0.2, 0.6], intensity: 0.75 },
            1.0,
            &mut dmx,
        );
        let allowed = |index: usize| {
            matches!(
                index,
                119
                    | 121..=123
                    | 199
                    | 201
                    | 204
                    | 212
                    | 214..=216
                    | 249
                    | 251
                    | 254
                    | 262
                    | 264..=266
            )
        };
        assert!(dmx.iter().enumerate().all(|(i, v)| allowed(i) || *v == 0));
        assert_eq!(dmx[199], 127, "first mover pan parks at midpoint");
        assert_eq!(dmx[201], 127, "first mover tilt parks at midpoint");
        assert_eq!(dmx[249], 127, "second mover pan parks at midpoint");
        assert_eq!(dmx[251], 127, "second mover tilt parks at midpoint");
        assert_eq!(dmx[109], 0, "RGB laser must stay off");
        assert_eq!(dmx[299], 0, "smoke must stay off");
        assert_eq!(dmx[309], 0, "second smoke must stay off");
        for base in [399, 419, 439, 459, 479] {
            assert_eq!(dmx[base], 0, "beam laser must stay off");
        }
        for base in fixture_patch::UV {
            assert_eq!(dmx[base - 1], 0, "UV must stay off");
        }
        for channel in [6, 8, 10, 13, 14, 15, 16, 17] {
            assert_eq!(
                dmx[fixture_patch::RGB_STROBE - 1 + channel - 1],
                0,
                "automatic input must not select a strobe/macro channel"
            );
        }
    }

    #[test]
    fn spatial_zones_reach_the_matching_room_fixtures() {
        let red = LightSample { rgb: [1.0, 0.0, 0.0], intensity: 1.0 };
        let green = LightSample { rgb: [0.0, 1.0, 0.0], intensity: 1.0 };
        let blue = LightSample { rgb: [0.0, 0.0, 1.0], intensity: 1.0 };
        let mut sample = SpatialLightSample::default();
        sample.zones[LightZone::Left as usize] = red;
        sample.zones[LightZone::Right as usize] = blue;
        sample.zones[LightZone::Center as usize] = green;
        let mut dmx = [0; DMX_LEN];
        render_spatial_video_lights(sample, 1.0, &mut dmx);

        assert_eq!(&dmx[fixture_patch::RGB_STROBE + 1..fixture_patch::RGB_STROBE + 4], &[0, 255, 0]);
        let left_rgb = fixture_patch::MOVING_WASHES[0] + 14;
        let right_rgb = fixture_patch::MOVING_WASHES[1] + 14;
        assert_eq!(&dmx[left_rgb..left_rgb + 3], &[255, 0, 0]);
        assert_eq!(&dmx[right_rgb..right_rgb + 3], &[0, 0, 255]);
    }

    #[test]
    fn analyzer_reads_bgra_and_blend_tracks_crossfade() {
        let red = bgra(255, 0, 0);
        let blue = bgra(0, 0, 255);
        let mut a = VideoLightAnalyzer::default();
        let mut b = VideoLightAnalyzer::default();
        let ra = a.push_bgra(&vec![red; 64]);
        let rb = b.push_bgra(&vec![blue; 64]);
        assert!(ra.rgb[0] > 0.99 && ra.rgb[2] < 0.01);
        assert!(rb.rgb[2] > 0.99 && rb.rgb[0] < 0.01);
        let middle = LightSample::blend(Some(ra), Some(rb), 0.5);
        assert!((middle.rgb[0] - 0.5).abs() < 0.01);
        assert!((middle.rgb[2] - 0.5).abs() < 0.01);
        let faded = LightSample::blend(Some(ra), None, 0.75);
        assert!((faded.intensity - ra.intensity * 0.25).abs() < 0.01);
    }

    #[test]
    fn analyzer_tracks_solid_primaries_in_every_zone() {
        for (color, primary) in [
            (bgra(255, 0, 0), 0),
            (bgra(0, 255, 0), 1),
            (bgra(0, 0, 255), 2),
        ] {
            let mut analyzer = VideoLightAnalyzer::default();
            let sample = analyzer.push_bgra_frame(&solid_frame(48, 27, color), 48, 27);
            assert!(sample.overall.intensity > 0.35);
            assert!(sample.overall.rgb[primary] > 0.98);
            for zone in LightZone::ALL {
                let zone = sample.zone(zone);
                assert!(zone.intensity > 0.30, "{zone:?}");
                assert!(zone.rgb[primary] > 0.98, "{zone:?}");
            }
        }
    }

    #[test]
    fn analyzer_separates_left_and_right_colors() {
        let (width, height) = (64, 36);
        let mut frame = vec![0; width * height];
        for y in 0..height {
            for x in 0..width {
                frame[y * width + x] = if x < width / 2 {
                    bgra(255, 0, 0)
                } else {
                    bgra(0, 0, 255)
                };
            }
        }
        let mut analyzer = VideoLightAnalyzer::default();
        let sample = analyzer.push_bgra_frame(&frame, width, height);
        let left = sample.zone(LightZone::Left);
        let right = sample.zone(LightZone::Right);
        assert!(left.rgb[0] > 0.9 && left.rgb[2] < 0.1, "{left:?}");
        assert!(right.rgb[2] > 0.9 && right.rgb[0] < 0.1, "{right:?}");
    }

    #[test]
    fn bright_small_subject_survives_dark_background() {
        let (width, height) = (80, 45);
        let mut frame = solid_frame(width, height, bgra(0, 0, 0));
        for y in 18..27 {
            for x in 34..46 {
                frame[y * width + x] = bgra(255, 32, 0);
            }
        }
        let mut analyzer = VideoLightAnalyzer::default();
        let sample = analyzer.push_bgra_frame(&frame, width, height);
        assert!(sample.overall.intensity > 0.08, "{sample:?}");
        assert!(sample.overall.rgb[0] > 0.9, "{sample:?}");
        assert!(sample.overall.rgb[2] < 0.15, "{sample:?}");
        assert!(sample.zone(LightZone::Center).intensity > 0.15, "{sample:?}");
    }

    #[test]
    fn white_and_gray_stay_neutral_and_black_stays_black() {
        for value in [64, 160, 255] {
            let mut analyzer = VideoLightAnalyzer::default();
            let sample = analyzer.push_bgra_frame(
                &solid_frame(32, 18, bgra(value, value, value)),
                32,
                18,
            );
            assert!(sample.overall.intensity > 0.0);
            assert_near(sample.overall.rgb[0], sample.overall.rgb[1], 0.001);
            assert_near(sample.overall.rgb[1], sample.overall.rgb[2], 0.001);
        }
        let mut analyzer = VideoLightAnalyzer::default();
        assert_eq!(
            analyzer.push_bgra_frame(&solid_frame(32, 18, bgra(0, 0, 0)), 32, 18),
            SpatialLightSample::default()
        );
    }

    #[test]
    fn scene_cut_attacks_fast_but_black_uses_configured_decay() {
        let config = VideoLightConfig {
            attack: 0.20,
            decay: 0.10,
            scene_cut_threshold: 0.25,
            scene_cut_attack: 0.90,
            ..VideoLightConfig::default()
        };
        let mut analyzer = VideoLightAnalyzer::new(config);
        let red = analyzer.push_bgra_frame(&solid_frame(40, 24, bgra(255, 0, 0)), 40, 24);
        let blue = analyzer.push_bgra_frame(&solid_frame(40, 24, bgra(0, 0, 255)), 40, 24);
        assert!(blue.overall.rgb[2] > 0.85, "{blue:?}");
        assert!(blue.overall.rgb[0] < 0.15, "{blue:?}");
        let dark = analyzer.push_bgra_frame(&solid_frame(40, 24, bgra(0, 0, 0)), 40, 24);
        assert_near(dark.overall.intensity, blue.overall.intensity * 0.9, 0.002);
        assert!(dark.overall.intensity > 0.0 && dark.overall.intensity < blue.overall.intensity);
        assert!(red.overall.intensity > 0.0);
    }

    #[test]
    fn crossfade_blends_each_spatial_zone() {
        let red = LightSample { rgb: [1.0, 0.0, 0.0], intensity: 0.8 };
        let blue = LightSample { rgb: [0.0, 0.0, 1.0], intensity: 0.4 };
        let mut a = SpatialLightSample::default();
        let mut b = SpatialLightSample::default();
        a.overall = red;
        a.zones[LightZone::Left as usize] = red;
        b.overall = blue;
        b.zones[LightZone::Left as usize] = blue;
        b.zones[LightZone::Right as usize] = blue;
        let mixed = SpatialLightSample::blend(Some(a), Some(b), 0.25);
        let left = mixed.zone(LightZone::Left);
        assert_near(left.rgb[0], 0.75, 0.001);
        assert_near(left.rgb[2], 0.25, 0.001);
        assert_near(left.intensity, 0.70, 0.001);
        assert_near(mixed.zone(LightZone::Right).intensity, 0.10, 0.001);
    }

    fn hazardous_performance_state() -> PerformanceState {
        PerformanceState {
            master: 1.0,
            ambilight_level: 0.0,
            rgb: ColorControl { rgb: [0.2, 0.4, 0.8], level: 1.0 },
            strobe: StrobeControl { level: 1.0, rate: 1.0 },
            movers: [
                MoverControl { position: [0.2, 0.3], rgb: [1.0, 0.0, 0.0], level: 1.0 },
                MoverControl { position: [0.7, 0.8], rgb: [0.0, 0.0, 1.0], level: 1.0 },
            ],
            hazards: HazardControl {
                rgb_laser: ColorControl { rgb: [1.0, 0.5, 0.25], level: 1.0 },
                beam_lasers: ColorControl { rgb: [0.0, 1.0, 0.0], level: 1.0 },
                beam_pattern: 0.5,
                smoke: [1.0, 1.0],
                uv: 1.0,
            },
            ..PerformanceState::default()
        }
    }

    fn hazardous_channels_are_zero(dmx: &[u8]) -> bool {
        dmx[fixture_patch::RGB_LASER - 1..fixture_patch::RGB_LASER + 9]
            .iter()
            .all(|value| *value == 0)
            && fixture_patch::BEAM_LASERS.iter().all(|base| {
                dmx[*base - 1..(*base - 1 + 12)]
                    .iter()
                    .all(|value| *value == 0)
            })
            && dmx[fixture_patch::SMOKE[0] - 1] == 0
            && dmx[fixture_patch::SMOKE[1] - 1] == 0
            && dmx[fixture_patch::SMOKE[1]] == 0
            && fixture_patch::UV.iter().all(|base| dmx[*base - 1] == 0)
    }

    #[test]
    fn hazardous_commands_require_both_arm_and_live_deadman() {
        let state = hazardous_performance_state();
        let mut dmx = [0; DMX_LEN];
        render_performance_lights(
            SpatialLightSample::default(),
            state,
            SafetySnapshot { control_live: true, deadman_live: true, arms: HazardArms::default() },
            1.0,
            PowerCaps::default(),
            [[0.5, 0.5]; 2],
            &mut dmx,
        );
        assert!(hazardous_channels_are_zero(&dmx));
        assert!(dmx[fixture_patch::RGB_STROBE - 1] > 0, "ordinary RGB remains live");

        render_performance_lights(
            SpatialLightSample::default(),
            state,
            SafetySnapshot {
                control_live: true,
                deadman_live: false,
                arms: HazardArms { lasers: true, smoke: true, uv: true },
            },
            1.0,
            PowerCaps::default(),
            [[0.5, 0.5]; 2],
            &mut dmx,
        );
        assert!(hazardous_channels_are_zero(&dmx));
        assert!(dmx[fixture_patch::RGB_STROBE - 1] > 0, "deadman timeout only drops hazards");
    }

    #[test]
    fn armed_commands_are_bounded_and_reach_only_their_fixture_groups() {
        let caps = PowerCaps {
            ambilight: 0.0,
            rgb: 0.4,
            movers: 0.5,
            strobe: 0.1,
            lasers: 0.2,
            smoke: 0.3,
            uv: 0.4,
        };
        let mut dmx = [0; DMX_LEN];
        render_performance_lights(
            SpatialLightSample::default(),
            hazardous_performance_state(),
            SafetySnapshot {
                control_live: true,
                deadman_live: true,
                arms: HazardArms { lasers: true, smoke: true, uv: true },
            },
            1.0,
            caps,
            [[0.5, 0.5]; 2],
            &mut dmx,
        );
        assert_eq!(dmx[fixture_patch::RGB_LASER - 1], 255, "explicit laser mode gate");
        assert!(dmx[fixture_patch::RGB_LASER] <= (caps.lasers * 255.0) as u8);
        for base in fixture_patch::BEAM_LASERS {
            assert!(dmx[base - 1] > 0 && dmx[base - 1] <= (caps.lasers * 255.0) as u8);
        }
        assert!(dmx[fixture_patch::SMOKE[0] - 1] > 0);
        assert!(dmx[fixture_patch::SMOKE[0] - 1] <= (caps.smoke * 255.0) as u8);
        for base in fixture_patch::UV {
            assert!(dmx[base - 1] > 0 && dmx[base - 1] <= (caps.uv * 255.0) as u8);
        }
        assert!(
            dmx[fixture_patch::RGB_STROBE - 1 + 5] <= (caps.strobe * 255.0) as u8,
            "strobe power cap applies after the requested level"
        );
        for base in fixture_patch::MOVING_WASHES {
            assert!(dmx[base - 1 + 5] <= (caps.movers * 255.0) as u8);
            assert!(dmx[base - 1 + 13] <= (caps.movers * 255.0) as u8);
        }
    }

    #[test]
    fn control_timeout_is_a_whole_room_blackout_with_safe_park() {
        let mut dmx = [0xa5; DMX_LEN];
        render_performance_lights(
            SpatialLightSample::uniform(LightSample {
                rgb: [1.0, 0.0, 0.0],
                intensity: 1.0,
            }),
            hazardous_performance_state(),
            SafetySnapshot {
                control_live: false,
                deadman_live: true,
                arms: HazardArms { lasers: true, smoke: true, uv: true },
            },
            1.0,
            PowerCaps::default(),
            [[0.4, 0.6], [0.3, 0.7]],
            &mut dmx,
        );
        let expected = [(199, 102), (201, 153), (249, 76), (251, 178)];
        for (index, value) in expected {
            assert_eq!(dmx[index], value);
        }
        assert!(dmx.iter().enumerate().all(|(index, value)| {
            expected.iter().any(|(parked, _)| *parked == index) || *value == 0
        }));
    }

    #[test]
    fn power_off_full_patch_is_black() {
        let mut dmx = [0u8; DMX_LEN];
        let state = ControllerState { fade: [1.0; 9], ..Default::default() };
        apply_dmx_mapping(&state, &ControllerButtons::default(), &mut dmx, 1.0);
        assert!(dmx.iter().all(|v| *v == 0));
    }

    #[test]
    fn artnet_worker_is_single_writer_and_blackouts_on_drop() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let target_addr = receiver.local_addr().unwrap().to_string();

        // Reserve an unused loopback address, then hand it to VideoArtNet as
        // this test's isolated single-writer lock.
        let lock_probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let lock_addr = lock_probe.local_addr().unwrap().to_string();
        drop(lock_probe);
        let config = ArtNetConfig {
            bind_addr: "127.0.0.1:0".to_string(),
            target_addr,
            lock_addr,
            master: 1.0,
        };

        let lighting = VideoArtNet::start(config.clone()).unwrap();
        assert!(VideoArtNet::start(config).is_err(), "a second controller must not start");
        lighting.set_sample(LightSample { rgb: [1.0, 0.0, 0.0], intensity: 1.0 });

        let mut packet = [0u8; ARTNET_PACKET_LEN];
        loop {
            let len = receiver.recv(&mut packet).unwrap();
            assert_eq!(len, ARTNET_PACKET_LEN);
            if packet[ARTNET_HEADER_LEN + 119] != 0 {
                break;
            }
        }
        assert_eq!(packet[ARTNET_HEADER_LEN + 299], 0, "smoke remains unreachable");
        drop(lighting);

        // Drain any already-buffered lit frame; Drop joins only after the
        // worker has emitted a black terminal scene with movers safely parked.
        loop {
            let len = receiver.recv(&mut packet).unwrap();
            assert_eq!(len, ARTNET_PACKET_LEN);
            let dmx = &packet[ARTNET_HEADER_LEN..];
            let parked = |index| matches!(index, 199 | 201 | 249 | 251);
            if dmx
                .iter()
                .enumerate()
                .all(|(index, value)| *value == if parked(index) { 127 } else { 0 })
            {
                break;
            }
        }
    }
}
