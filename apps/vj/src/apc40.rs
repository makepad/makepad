//! Akai APC40 MkII performance mapping.
//!
//! This module is deliberately UI- and platform-free: raw three-byte MIDI
//! messages become semantic performance actions, and semantic pad states
//! become delta-compressed LED messages. The app adapter only selects the
//! matching Makepad MIDI ports and dispatches actions to its existing cue,
//! deck, and SFX engines.

pub const PAD_COUNT: usize = 40;
pub const PAGE_SIZE: usize = PAD_COUNT;

pub const NOTE_PAN: u8 = 0x57;
pub const NOTE_SENDS: u8 = 0x58;
// NOTE 0x59 (USER) is automate lighting POWER. Do not steal it for a VJ surface.
pub const NOTE_PLAY: u8 = 0x5b;
pub const NOTE_STOP: u8 = 0x5c;
pub const NOTE_UP: u8 = 0x5e;
pub const NOTE_DOWN: u8 = 0x5f;
pub const NOTE_RIGHT: u8 = 0x60;
pub const NOTE_LEFT: u8 = 0x61;
pub const CC_MASTER: u8 = 0x0e;
pub const CC_CROSSFADER: u8 = 0x0f;
/// Channel volume faders: CC 7 on MIDI channels 0..7.
pub const CC_CHANNEL_FADER: u8 = 0x07;
/// Top knob row, CC 0x10..0x17.
pub const CC_TRACK_KNOB_FIRST: u8 = 0x10;
/// Bottom (device) knob row, CC 0x30..0x37.
pub const CC_DEVICE_KNOB_FIRST: u8 = 0x30;
/// Knobs per row.
pub const KNOB_ROW: usize = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApcSurface {
    #[default]
    Video,
    Music,
    Sfx,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ApcAction {
    Pad {
        surface: ApcSurface,
        pad: usize,
        index: usize,
        pressed: bool,
    },
    Surface(ApcSurface),
    VideoPlayPause,
    VideoStop,
    Master(f32),
    Crossfader(f32),
    /// One of the eight channel volume faders.
    ChannelFader { channel: usize, value: f32 },
    /// Top knob row.
    TrackKnob { index: usize, value: f32 },
    /// Bottom (device) knob row.
    DeviceKnob { index: usize, value: f32 },
    BankChanged,
}

#[derive(Default)]
pub struct Apc40State {
    pub surface: ApcSurface,
    pub bank: usize,
}

impl Apc40State {
    pub fn decode(&mut self, data: [u8; 3]) -> Option<ApcAction> {
        let status = data[0] >> 4;
        let is_note = status == 0x8 || status == 0x9;
        let pressed = status == 0x9 && data[2] != 0;
        if is_note {
            let note = data[1];
            if (note as usize) < PAD_COUNT {
                return Some(ApcAction::Pad {
                    surface: self.surface,
                    pad: note as usize,
                    index: self.bank.saturating_add(note as usize),
                    pressed,
                });
            }
            if !pressed {
                return None;
            }
            return match note {
                NOTE_PAN => {
                    self.surface = ApcSurface::Video;
                    self.bank = 0;
                    Some(ApcAction::Surface(self.surface))
                }
                NOTE_SENDS => {
                    self.surface = ApcSurface::Music;
                    self.bank = 0;
                    Some(ApcAction::Surface(self.surface))
                }
                NOTE_PLAY => Some(ApcAction::VideoPlayPause),
                NOTE_STOP => Some(ApcAction::VideoStop),
                // Video: the clip grid is a horizontal column strip (bank =
                // first visible column) — ◀ ▶ one column, ▲ ▼ a page of 8.
                // Music/SFX: flat lists — ◀ ▶ a row of 8, ▲ ▼ a page of 40.
                NOTE_UP => {
                    self.bank = self.bank.saturating_sub(self.page_step());
                    Some(ApcAction::BankChanged)
                }
                NOTE_DOWN => {
                    self.bank = self.bank.saturating_add(self.page_step());
                    Some(ApcAction::BankChanged)
                }
                NOTE_LEFT => {
                    self.bank = self.bank.saturating_sub(self.row_step());
                    Some(ApcAction::BankChanged)
                }
                NOTE_RIGHT => {
                    self.bank = self.bank.saturating_add(self.row_step());
                    Some(ApcAction::BankChanged)
                }
                _ => None,
            };
        }
        if status == 0xb {
            let value = data[2] as f32 / 127.0;
            let channel = (data[0] & 0x0f) as usize;
            return match data[1] {
                CC_MASTER => Some(ApcAction::Master(value)),
                CC_CROSSFADER => Some(ApcAction::Crossfader(value)),
                CC_CHANNEL_FADER if channel < KNOB_ROW => {
                    Some(ApcAction::ChannelFader { channel, value })
                }
                cc if (CC_TRACK_KNOB_FIRST..CC_TRACK_KNOB_FIRST + KNOB_ROW as u8)
                    .contains(&cc) =>
                {
                    Some(ApcAction::TrackKnob {
                        index: (cc - CC_TRACK_KNOB_FIRST) as usize,
                        value,
                    })
                }
                cc if (CC_DEVICE_KNOB_FIRST..CC_DEVICE_KNOB_FIRST + KNOB_ROW as u8)
                    .contains(&cc) =>
                {
                    Some(ApcAction::DeviceKnob {
                        index: (cc - CC_DEVICE_KNOB_FIRST) as usize,
                        value,
                    })
                }
                _ => None,
            };
        }
        None
    }

    fn row_step(&self) -> usize {
        match self.surface {
            ApcSurface::Video => 1,
            ApcSurface::Music | ApcSurface::Sfx => 8,
        }
    }

    fn page_step(&self) -> usize {
        match self.surface {
            ApcSurface::Video => 8,
            ApcSurface::Music | ApcSurface::Sfx => PAGE_SIZE,
        }
    }

    pub fn clamp_bank(&mut self, item_count: usize) {
        if item_count == 0 {
            self.bank = 0;
            return;
        }
        let last_page = ((item_count - 1) / PAGE_SIZE) * PAGE_SIZE;
        self.bank = self.bank.min(last_page);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadLed {
    #[default]
    Off,
    /// Resolved, no thumbnail colour known yet.
    Ready,
    /// Cued / resolving (no colour known).
    Queued,
    /// On air (no colour known).
    Live,
    Failed,
    /// Resolved: the clip's thumbnail colour (palette velocity).
    Color(u8),
    /// Cued next: the clip's colour, blinking.
    NextColor(u8),
    /// On air: the clip's colour, pulsing.
    LiveColor(u8),
}

/// Which Akai surface is attached. The two mk2 devices share the 128-colour
/// palette EXACTLY (verified entry-by-entry against both communications
/// protocol documents) but disagree about what a MIDI channel means, so a
/// message built for one is mis-lit on the other.
///
/// APC40 mkII (protocol v1.2, "RGB LEDs Type" table):
///   ch 0 = primary colour (solid), 1–5 oneshot 1/24…1/2,
///   6–10 pulsing 1/24…1/2, 11–15 blinking 1/24…1/2.
/// APC mini mk2 (protocol v1.0, "RGB LED Behavior" table):
///   ch 0–6 = solid at 10/25/50/65/75/90/100 % brightness,
///   7–10 pulsing 1/16…1/2, 11–15 blinking 1/24…1/2.
///
/// So a solid colour is channel 0 on the APC40 mkII but channel 6 on the
/// mini — sending channel 0 to a mini lights every pad at 10 % brightness,
/// which reads as a washed-out colour that does not match its tile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApcModel {
    #[default]
    Apc40Mk2,
    ApcMiniMk2,
}

impl ApcModel {
    /// Channel for a solid, full-brightness colour.
    pub fn solid_channel(self) -> u8 {
        match self {
            Self::Apc40Mk2 => 0,  // "Primary Color"
            Self::ApcMiniMk2 => 6, // "On 100% brightness"
        }
    }

    /// Channel for pulsing at 1/8 (both docs put 1/8 on channel 8).
    pub fn pulse_channel(self) -> u8 {
        8
    }

    /// Channel for blinking at 1/8 (both docs put 1/8 on channel 13).
    pub fn blink_channel(self) -> u8 {
        13
    }

    /// Note number for VJ pad `index` (0 = top-left of the 5×8 surface).
    ///
    /// The APC40 mkII's 5×8 clip grid is notes 0..39 in reading order. The
    /// mini mk2's 8×8 grid numbers rows BOTTOM-up (note 0 = bottom-left,
    /// 56 = top-left), so the VJ's five rows are laid on its top five and
    /// the bottom three stay dark.
    pub fn pad_note(self, index: usize) -> u8 {
        match self {
            Self::Apc40Mk2 => index as u8,
            Self::ApcMiniMk2 => {
                let (row, col) = (index / 8, index % 8);
                ((7 - row.min(7)) * 8 + col) as u8
            }
        }
    }

    /// Only the mini mk2 protocol defines the RGB SysEx; the APC40 mkII has
    /// no such message and stays on the palette.
    pub fn supports_rgb_sysex(self) -> bool {
        matches!(self, Self::ApcMiniMk2)
    }

    /// Product model ID byte of the RGB lighting SysEx.
    fn sysex_model_id(self) -> u8 {
        match self {
            Self::ApcMiniMk2 => 0x4F,
            Self::Apc40Mk2 => 0x29,
        }
    }
}

/// Exact per-pad RGB, bypassing the 128-colour palette:
/// `F0 47 7F <model> 24 <len MSB> <len LSB> <start pad> <end pad>
///  <R MSB> <R LSB> <G MSB> <G LSB> <B MSB> <B LSB> … F7`
/// (APC mini mk2 protocol v1.0, "RGB LED Color Lighting"). Each 8-bit
/// channel is split into two 7-bit halves; the length counts the bytes
/// after the length field itself, and one message may carry several pad
/// ranges. Blink/pulse are palette-only per the protocol, so an RGB pad is
/// always solid.
pub fn rgb_sysex(model: ApcModel, spans: &[(u8, u8, (u8, u8, u8))]) -> Option<Vec<u8>> {
    if !model.supports_rgb_sysex() || spans.is_empty() {
        return None;
    }
    let mut body = Vec::with_capacity(spans.len() * 8);
    for (start, end, (r, g, b)) in spans.iter().copied() {
        body.push(start & 0x3f);
        body.push(end & 0x3f);
        for c in [r, g, b] {
            body.push(c >> 7); // MSB (always 0 or 1 for an 8-bit value)
            body.push(c & 0x7f); // LSB
        }
    }
    let len = body.len();
    let mut out = Vec::with_capacity(len + 8);
    out.extend_from_slice(&[0xF0, 0x47, 0x7F, model.sysex_model_id(), 0x24]);
    out.push(((len >> 7) & 0x7f) as u8);
    out.push((len & 0x7f) as u8);
    out.extend_from_slice(&body);
    out.push(0xF7);
    Some(out)
}

impl PadLed {
    /// Palette velocities used when a tile has no colour of its own:
    /// blue 41 (ready), amber 126 (cued), green 21 (live), red 5 (failed).
    fn message(self, model: ApcModel, note: u8) -> [u8; 3] {
        let solid = model.solid_channel();
        let (channel, velocity) = match self {
            Self::Off => (solid, 0),
            Self::Ready => (solid, 41),
            Self::Queued => (solid, 126),
            Self::Live => (model.pulse_channel(), 21),
            Self::Failed => (solid, 5),
            Self::Color(v) => (solid, v & 0x7f),
            Self::NextColor(v) => (model.blink_channel(), v & 0x7f),
            Self::LiveColor(v) => (model.pulse_channel(), v & 0x7f),
        };
        [0x90 | channel, note, velocity]
    }
}

/// The APC40 MkII's fixed 128-entry pad palette (velocity → RGB), the
/// Akai APC-family clip-colour table. Index = velocity.
pub const PAD_PALETTE: [u32; 128] = [
    0x000000, 0x1E1E1E, 0x7F7F7F, 0xFFFFFF, 0xFF4C4C, 0xFF0000, 0x590000, 0x190000,
    0xFFBD6C, 0xFF5400, 0x591D00, 0x271B00, 0xFFFF4C, 0xFFFF00, 0x595900, 0x191900,
    0x88FF4C, 0x54FF00, 0x1D5900, 0x142B00, 0x4CFF4C, 0x00FF00, 0x005900, 0x001900,
    0x4CFF5E, 0x00FF19, 0x00590D, 0x001902, 0x4CFF88, 0x00FF55, 0x00591D, 0x001F12,
    0x4CFFB7, 0x00FF99, 0x005935, 0x001912, 0x4CC3FF, 0x00A9FF, 0x004152, 0x001019,
    0x4C88FF, 0x0055FF, 0x001D59, 0x000819, 0x4C4CFF, 0x0000FF, 0x000059, 0x000019,
    0x874CFF, 0x5400FF, 0x190064, 0x0F0030, 0xFF4CFF, 0xFF00FF, 0x590059, 0x190019,
    0xFF4C87, 0xFF0054, 0x59001D, 0x220013, 0xFF1500, 0x993500, 0x795100, 0x436400,
    0x033900, 0x005735, 0x00547F, 0x0000FF, 0x00454F, 0x2500CC, 0x7F7F7F, 0x202020,
    0xFF0000, 0xBDFF2D, 0xAFED06, 0x64FF09, 0x108B00, 0x00FF87, 0x00A9FF, 0x002AFF,
    0x3F00FF, 0x7A00FF, 0xB21A7D, 0x402100, 0xFF4A00, 0x88E106, 0x72FF15, 0x00FF00,
    0x3BFF26, 0x59FF71, 0x38FFCC, 0x5B8AFF, 0x3151C6, 0x877FE9, 0xD31DFF, 0xFF005D,
    0xFF7F00, 0xB9B000, 0x90FF00, 0x835D07, 0x392B00, 0x144C10, 0x0D5038, 0x15152A,
    0x16205A, 0x693C1C, 0xA8000A, 0xDE513D, 0xD86A1C, 0xFFE126, 0x9EE12F, 0x67B50F,
    0x1E1E30, 0xDCFF6B, 0x80FFBD, 0x9A99FF, 0x8E66FF, 0x404040, 0x757575, 0xE0FFFF,
    0xA00000, 0x350000, 0x1AD000, 0x074200, 0xB9B000, 0x3F3100, 0xB35F00, 0x4B1502,
];

/// Hue (0–360), saturation and value (0–1) of an 8-bit RGB colour.
fn hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max <= 0.0 { 0.0 } else { d / max };
    (h, s, max)
}

/// Velocities of the palette's neutral greys (dark → light).
const GREY_PADS: [u8; 7] = [1, 71, 117, 118, 2, 119, 3];

/// The palette velocity that reads as an RGB colour — matched by HUE first
/// (a muted teal must land on a teal pad, not on the nearest-by-RGB grey or
/// lavender), then saturation and brightness. Greys pick among the greys.
/// Never 0 (off): a clip always gets a lit pad.
pub fn palette_velocity(r: u8, g: u8, b: u8) -> u8 {
    let (h, s, v) = hsv(r, g, b);
    if s < 0.18 {
        let lum = (r as f32 * 0.3 + g as f32 * 0.59 + b as f32 * 0.11) / 255.0;
        let mut best = GREY_PADS[0];
        let mut best_d = f32::MAX;
        for &pad in &GREY_PADS {
            let rgb = PAD_PALETTE[pad as usize];
            let pl = (((rgb >> 16) & 0xff) as f32 * 0.3
                + ((rgb >> 8) & 0xff) as f32 * 0.59
                + (rgb & 0xff) as f32 * 0.11)
                / 255.0;
            let d = (pl - lum).abs();
            if d < best_d {
                best_d = d;
                best = pad;
            }
        }
        return best;
    }
    let mut best = 1u8;
    let mut best_d = f32::MAX;
    for (i, rgb) in PAD_PALETTE.iter().enumerate().skip(1) {
        let (ph, ps, pv) = hsv((rgb >> 16) as u8, (rgb >> 8) as u8, *rgb as u8);
        if ps < 0.3 {
            continue; // a chromatic input never lands on a grey pad
        }
        let dh = (h - ph).abs();
        let dh = dh.min(360.0 - dh) / 180.0;
        let d = dh * dh * 8.0 + (s - ps) * (s - ps) + (v - pv) * (v - pv);
        if d < best_d {
            best_d = d;
            best = i as u8;
        }
    }
    best
}

/// Hue buckets in the dominant-colour histogram (15° each).
const HUE_BUCKETS: usize = 24;
/// A pixel must be at least this saturated / lit to vote for a hue.
const VOTE_MIN_SAT: f32 = 0.22;
const VOTE_MIN_VAL: f32 = 0.16;
/// The winning hue must cover this share of the LIT PIXELS (not of the
/// votes — 40 red roof texels on a grey Kenney house are 100 % of the votes
/// but 4 % of the picture, and must not repaint the whole pad).
const HUE_MIN_SHARE: f32 = 0.10;

/// The colour a thumbnail "reads as": the DOMINANT saturated hue, not the
/// mean.
///
/// A mean is what made Doom sprites and grey Kenney props land on muddy,
/// interchangeable pads: averaging a mostly-dark brown sprite with its
/// bright highlights gives a colour that is in the image nowhere. Instead
/// every lit, saturated pixel votes for a 15° hue bucket (weighted by
/// saturation × value, so vivid pixels count for more than washed ones);
/// the winning bucket's own pixels are then averaged, which keeps a real
/// colour from the picture. Images with no dominant hue fall back to the
/// luminance mean and stay grey — the palette has greys, and a grey prop
/// should light a grey pad rather than a random hue.
///
/// `None` for an empty/transparent/black image (the pad keeps `Ready`).
pub fn thumb_color(bgra: &[u32]) -> Option<(u8, u8, u8)> {
    let mut votes = [0.0f32; HUE_BUCKETS];
    let mut sums = [(0u64, 0u64, 0u64, 0u64); HUE_BUCKETS];
    let (mut sr, mut sg, mut sb, mut n) = (0u64, 0u64, 0u64, 0u64);
    for px in bgra {
        if (px >> 24) < 64 {
            continue; // transparent
        }
        let r = (px >> 16) as u8;
        let g = (px >> 8) as u8;
        let b = *px as u8;
        let lum = (r as u32 * 54 + g as u32 * 183 + b as u32 * 19) >> 8;
        if lum < 40 {
            continue; // studio backdrop / sprite shadow
        }
        sr += r as u64;
        sg += g as u64;
        sb += b as u64;
        n += 1;
        let (h, s, v) = hsv(r, g, b);
        if s < VOTE_MIN_SAT || v < VOTE_MIN_VAL {
            continue;
        }
        let bucket = ((h / 360.0 * HUE_BUCKETS as f32) as usize).min(HUE_BUCKETS - 1);
        votes[bucket] += s * v;
        let acc = &mut sums[bucket];
        acc.0 += r as u64;
        acc.1 += g as u64;
        acc.2 += b as u64;
        acc.3 += 1;
    }
    if n == 0 {
        return None;
    }
    let best = votes
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let lit = sums[best].3;
    if lit > 0 && lit as f32 >= n as f32 * HUE_MIN_SHARE {
        let (r, g, b) = (
            (sums[best].0 / lit) as f32,
            (sums[best].1 / lit) as f32,
            (sums[best].2 / lit) as f32,
        );
        // Modest lift: the LED should read as this hue at pad size, but the
        // hue itself is the picture's, never invented.
        let peak = r.max(g).max(b).max(1.0);
        let gain = (255.0 * 0.9 / peak).clamp(1.0, 2.2);
        return Some((
            (r * gain).min(255.0) as u8,
            (g * gain).min(255.0) as u8,
            (b * gain).min(255.0) as u8,
        ));
    }
    // No dominant hue: an honest grey at the image's own brightness.
    let mean = ((sr + sg + sb) / (3 * n)) as f32;
    let l = (mean * 1.25).clamp(48.0, 230.0) as u8;
    Some((l, l, l))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedFrame {
    pub pads: [PadLed; PAD_COUNT],
    pub surface: ApcSurface,
    pub video_playing: bool,
}

impl Default for LedFrame {
    fn default() -> Self {
        Self {
            pads: [PadLed::Off; PAD_COUNT],
            surface: ApcSurface::Video,
            video_playing: false,
        }
    }
}

#[derive(Default)]
pub struct LedDiff {
    last: Option<LedFrame>,
    pub model: ApcModel,
}

impl LedDiff {
    pub fn set_model(&mut self, model: ApcModel) {
        if self.model != model {
            self.model = model;
            self.last = None; // every pad must be re-sent in the new dialect
        }
    }

    pub fn update(&mut self, next: LedFrame) -> Vec<[u8; 3]> {
        let mut out = Vec::new();
        for (index, state) in next.pads.iter().copied().enumerate() {
            if self.last.as_ref().is_none_or(|last| last.pads[index] != state) {
                out.push(state.message(self.model, self.model.pad_note(index)));
            }
        }
        for (surface, note) in [
            (ApcSurface::Video, NOTE_PAN),
            (ApcSurface::Music, NOTE_SENDS),
        ] {
            let changed = self.last.as_ref().is_none_or(|last| last.surface != next.surface);
            if changed {
                out.push([0x90, note, if next.surface == surface { 127 } else { 0 }]);
            }
        }
        if self
            .last
            .as_ref()
            .is_none_or(|last| last.video_playing != next.video_playing)
        {
            out.push([0x90, NOTE_PLAY, if next.video_playing { 127 } else { 0 }]);
        }
        self.last = Some(next);
        out
    }

    pub fn invalidate(&mut self) {
        self.last = None;
    }
}

/// Which mk2 surface (if any) a MIDI port name names. The original APC40
/// and APC mini (mk1) use different channel/note maps and are never
/// silently attached to this protocol adapter.
pub fn apc_model_for_port(name: &str) -> Option<ApcModel> {
    let compact: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if compact.contains("apc40mkii") || compact.contains("apc40mk2") {
        return Some(ApcModel::Apc40Mk2);
    }
    if compact.contains("apcminimkii") || compact.contains("apcminimk2") {
        return Some(ApcModel::ApcMiniMk2);
    }
    None
}

pub fn is_apc40_port(name: &str) -> bool {
    apc_model_for_port(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_faders_and_both_knob_rows_decode() {
        let mut state = Apc40State::default();
        // Channel faders carry their strip in the MIDI channel nibble.
        assert_eq!(
            state.decode([0xb0, CC_CHANNEL_FADER, 127]),
            Some(ApcAction::ChannelFader { channel: 0, value: 1.0 })
        );
        assert_eq!(
            state.decode([0xb1, CC_CHANNEL_FADER, 0]),
            Some(ApcAction::ChannelFader { channel: 1, value: 0.0 })
        );
        // Top row.
        assert_eq!(
            state.decode([0xb0, CC_TRACK_KNOB_FIRST, 64]),
            Some(ApcAction::TrackKnob { index: 0, value: 64.0 / 127.0 })
        );
        assert_eq!(
            state.decode([0xb0, CC_TRACK_KNOB_FIRST + 7, 127]),
            Some(ApcAction::TrackKnob { index: 7, value: 1.0 })
        );
        // Bottom row.
        assert_eq!(
            state.decode([0xb0, CC_DEVICE_KNOB_FIRST + 3, 32]),
            Some(ApcAction::DeviceKnob { index: 3, value: 32.0 / 127.0 })
        );
        // The master and crossfader keep their own meanings.
        assert_eq!(state.decode([0xb0, CC_MASTER, 127]), Some(ApcAction::Master(1.0)));
        assert_eq!(
            state.decode([0xb0, CC_CROSSFADER, 0]),
            Some(ApcAction::Crossfader(0.0))
        );
        // An unmapped controller stays unmapped.
        assert_eq!(state.decode([0xb0, 0x22, 100]), None);
    }

    #[test]
    fn pad_press_release_and_banking_are_exact() {
        let mut state = Apc40State::default();
        assert_eq!(
            state.decode([0x90, 39, 127]),
            Some(ApcAction::Pad {
                surface: ApcSurface::Video,
                pad: 39,
                index: 39,
                pressed: true,
            })
        );
        // Video strip: ▼ = a page of 8 columns, ▶ = one column.
        assert_eq!(state.decode([0x90, NOTE_DOWN, 127]), Some(ApcAction::BankChanged));
        assert_eq!(state.bank, 8);
        assert_eq!(state.decode([0x90, NOTE_RIGHT, 127]), Some(ApcAction::BankChanged));
        assert_eq!(state.bank, 9);
        assert_eq!(state.decode([0x90, NOTE_UP, 127]), Some(ApcAction::BankChanged));
        assert_eq!(state.bank, 1);
        assert_eq!(
            state.decode([0x80, 0, 127]),
            Some(ApcAction::Pad {
                surface: ApcSurface::Video,
                pad: 0,
                index: 1,
                pressed: false,
            })
        );
        // Flat lists (SFX): ▼ = 40, ▶ = 8.
        state.decode([0x90, NOTE_SENDS, 127]);
        state.decode([0x90, NOTE_SENDS, 127]);
        state.surface = ApcSurface::Sfx;
        state.bank = 0;
        state.decode([0x90, NOTE_DOWN, 127]);
        assert_eq!(state.bank, 40);
        state.decode([0x90, NOTE_RIGHT, 127]);
        assert_eq!(state.bank, 48);
        state.clamp_bank(41);
        assert_eq!(state.bank, 40);
        state.clamp_bank(40);
        assert_eq!(state.bank, 0);
    }

    #[test]
    fn thumbnail_colours_land_on_lit_palette_pads() {
        // Pure red → the red pad; dark backdrop pixels are ignored.
        let mut px = vec![0xff101010u32; 200];
        px.extend(std::iter::repeat(0xffd02020u32).take(50));
        let (r, g, b) = thumb_color(&px).expect("colour");
        assert!(r > g && r > b);
        let v = palette_velocity(r, g, b);
        assert!(v != 0);
        let rgb = PAD_PALETTE[v as usize];
        let (pr, pg, pb) = (((rgb >> 16) & 0xff) as i32, ((rgb >> 8) & 0xff) as i32, (rgb & 0xff) as i32);
        assert!(pr > 0xc0 && pg < 0x60 && pb < 0x60, "{rgb:06x}");
        // Transparent / black thumbnails give no colour (pad keeps Ready).
        assert!(thumb_color(&[0u32; 16]).is_none());
        assert!(thumb_color(&[0xff000000u32; 16]).is_none());
        // Grey stays grey (never a random hue).
        let (r, g, b) = thumb_color(&[0xff808080u32; 16]).unwrap();
        assert!(r == g && g == b);
        assert!(matches!(palette_velocity(r, g, b), 1 | 2 | 70 | 71 | 117 | 118));
    }

    #[test]
    fn mode_and_continuous_controls_decode() {
        let mut state = Apc40State::default();
        assert_eq!(
            state.decode([0x90, NOTE_SENDS, 127]),
            Some(ApcAction::Surface(ApcSurface::Music))
        );
        assert_eq!(state.surface, ApcSurface::Music);
        assert_eq!(state.decode([0xb0, CC_CROSSFADER, 127]), Some(ApcAction::Crossfader(1.0)));
        assert_eq!(state.decode([0xb0, CC_MASTER, 0]), Some(ApcAction::Master(0.0)));
    }

    #[test]
    fn pad_action_captures_surface_before_later_batch_messages() {
        let mut state = Apc40State::default();
        let pad = state.decode([0x90, 7, 127]);
        assert_eq!(
            state.decode([0x90, NOTE_SENDS, 127]),
            Some(ApcAction::Surface(ApcSurface::Music))
        );
        assert_eq!(
            pad,
            Some(ApcAction::Pad {
                surface: ApcSurface::Video,
                pad: 7,
                index: 7,
                pressed: true,
            })
        );
    }

    #[test]
    fn led_output_is_delta_compressed_and_live_pulses() {
        let mut diff = LedDiff::default();
        let mut frame = LedFrame::default();
        frame.pads[3] = PadLed::Ready;
        frame.pads[4] = PadLed::Live;
        let first = diff.update(frame.clone());
        assert!(first.contains(&[0x90, 3, 41]));
        assert!(first.contains(&[0x98, 4, 21]));
        assert!(diff.update(frame.clone()).is_empty());
        frame.pads[3] = PadLed::Failed;
        assert_eq!(diff.update(frame), vec![[0x90, 3, 5]]);
    }

    #[test]
    fn solid_channel_follows_the_device_protocol() {
        // APC40 mkII: channel 0 IS the primary (solid) colour.
        let mut diff = LedDiff::default();
        diff.set_model(ApcModel::Apc40Mk2);
        let mut frame = LedFrame::default();
        frame.pads[0] = PadLed::Color(21);
        frame.pads[1] = PadLed::NextColor(21);
        frame.pads[2] = PadLed::LiveColor(21);
        let out = diff.update(frame.clone());
        assert!(out.contains(&[0x90, 0, 21]), "solid on ch 0: {out:?}");
        assert!(out.contains(&[0x9d, 1, 21]), "blink 1/8 on ch 13");
        assert!(out.contains(&[0x98, 2, 21]), "pulse 1/8 on ch 8");
        // APC mini mk2: channel 0 is only 10 % brightness — a solid colour
        // has to go out on channel 6, and its grid numbers rows bottom-up.
        let mut diff = LedDiff::default();
        diff.set_model(ApcModel::ApcMiniMk2);
        let out = diff.update(frame);
        assert!(out.contains(&[0x96, 56, 21]), "solid 100% on ch 6: {out:?}");
        assert!(out.contains(&[0x9d, 57, 21]));
        assert!(out.contains(&[0x98, 58, 21]));
        // Row 1 of the VJ surface is the mini's row 6 (notes 48..55).
        assert_eq!(ApcModel::ApcMiniMk2.pad_note(8), 48);
        assert_eq!(ApcModel::ApcMiniMk2.pad_note(39), 31);
        assert_eq!(ApcModel::Apc40Mk2.pad_note(39), 39);
    }

    #[test]
    fn rgb_sysex_matches_the_published_frame() {
        // F0 47 7F 4F 24 <len MSB> <len LSB> <start> <end> R.. G.. B.. F7
        let msg = rgb_sysex(ApcModel::ApcMiniMk2, &[(0, 0, (0xFF, 0x80, 0x00))]).unwrap();
        assert_eq!(
            msg,
            vec![
                0xF0, 0x47, 0x7F, 0x4F, 0x24, 0x00, 0x08, 0x00, 0x00, 0x01, 0x7F, 0x01,
                0x00, 0x00, 0x00, 0xF7,
            ]
        );
        // Every payload byte is 7-bit clean, and the length counts the bytes
        // after the length field.
        let msg = rgb_sysex(
            ApcModel::ApcMiniMk2,
            &[(0, 7, (0x12, 0x34, 0x56)), (8, 15, (0xFF, 0xFF, 0xFF))],
        )
        .unwrap();
        assert_eq!(msg[5..7], [0x00, 0x10], "16 payload bytes");
        assert_eq!(msg.len(), 16 + 8);
        assert!(msg[5..msg.len() - 1].iter().all(|b| *b < 0x80));
        assert_eq!(*msg.last().unwrap(), 0xF7);
        // The APC40 mkII protocol has no RGB SysEx: never send it one.
        assert!(rgb_sysex(ApcModel::Apc40Mk2, &[(0, 0, (1, 2, 3))]).is_none());
        assert!(rgb_sysex(ApcModel::ApcMiniMk2, &[]).is_none());
    }

    #[test]
    fn port_match_is_specific_but_tolerates_spacing() {
        assert_eq!(apc_model_for_port("Akai APC40 mkII"), Some(ApcModel::Apc40Mk2));
        assert_eq!(apc_model_for_port("APC 40 mk2 Control"), Some(ApcModel::Apc40Mk2));
        assert_eq!(
            apc_model_for_port("Akai APC mini mk2"),
            Some(ApcModel::ApcMiniMk2)
        );
        assert_eq!(apc_model_for_port("APC mini mkII"), Some(ApcModel::ApcMiniMk2));
        assert!(is_apc40_port("Akai APC40 mkII"));
        // mk1 hardware speaks a different dialect.
        assert!(!is_apc40_port("Akai APC40"));
        assert!(!is_apc40_port("Akai APC mini"));
        assert!(!is_apc40_port("Launchpad Pro"));
    }

    /// The palette is the hardware's, so the only honest check is that a
    /// picture's dominant colour reaches a pad of the SAME hue family.
    fn pad_hue(v: u8) -> (f32, f32, f32) {
        let rgb = PAD_PALETTE[v as usize];
        hsv((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    #[test]
    fn dominant_colour_beats_the_mean_on_real_tile_shapes() {
        // A Doom sprite: mostly dark backdrop + transparent, a brown body
        // and a few bright red pixels. The mean of this is mud; the pad
        // must read as the body colour.
        let mut sprite = vec![0u32; 900]; // transparent
        sprite.extend(std::iter::repeat(0xff10_1014u32).take(400)); // near-black
        sprite.extend(std::iter::repeat(0xff8a_5a2au32).take(260)); // brown body
        sprite.extend(std::iter::repeat(0xffd0_2020u32).take(40)); // red trim
        let (r, g, b) = thumb_color(&sprite).expect("a lit sprite has a colour");
        let (h, s, _v) = hsv(r, g, b);
        assert!(s > 0.35, "the body colour must survive as a colour: {r},{g},{b}");
        assert!(
            (10.0..50.0).contains(&h),
            "brown/orange body, not the red trim or a grey mean: h={h}"
        );
        let v = palette_velocity(r, g, b);
        let (ph, ps, _) = pad_hue(v);
        assert!(ps > 0.3 && (0.0..60.0).contains(&ph), "pad #{:06X}", PAD_PALETTE[v as usize]);

        // A grey Kenney house with a small red roof: still reads grey (a
        // handful of coloured texels must not repaint the whole pad).
        let mut house = vec![0xff9a_9a9au32; 900];
        house.extend(std::iter::repeat(0xffcc_2222u32).take(40));
        let (r, g, b) = thumb_color(&house).unwrap();
        assert_eq!((r, g, b), (r, r, r), "a grey prop lights a grey pad");
        assert!(matches!(palette_velocity(r, g, b), 1 | 2 | 3 | 70 | 71 | 117 | 118 | 119));

        // A saturated green generated mesh on the studio backdrop.
        let mut mesh = vec![0xff1a_1f29u32; 700];
        mesh.extend(std::iter::repeat(0xff2f_c04au32).take(300));
        let (r, g, b) = thumb_color(&mesh).unwrap();
        assert!(g > r && g > b, "green stays green: {r},{g},{b}");
        let (ph, ps, _) = pad_hue(palette_velocity(r, g, b));
        assert!(ps > 0.3 && (80.0..170.0).contains(&ph), "green pad, got h={ph}");
    }
}

