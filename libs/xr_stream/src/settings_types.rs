//! The stream's settings the user picks (the caption's XR dropdown): its anti-aliasing and
//! the headset's view.

/// The stream's anti-aliasing, one of [`STREAM_AA_MODES`] (the app's setting, applied live from
/// the next frame): hardware MSAA on the eye pass (`msaa` samples per pixel, resolved before
/// the encoder's pack), FXAA over the resolved eyes, supersampled eyes filtered down into the
/// encoded eye size (`supersample` rendered pixels per encoded pixel per axis), or a mix.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamAa {
    /// Samples per pixel of the eye pass (1: no MSAA; a count the GPU lacks: the most it has
    /// below that, shown on the stats panel).
    pub msaa: u32,
    pub fxaa: bool,
    pub supersample: f32,
    /// Its id (the settings file) and its name for the user.
    pub id: &'static str,
    pub name: &'static str,
}

const fn aa(msaa: u32, fxaa: bool, supersample: f32, id: &'static str, name: &'static str) -> StreamAa {
    StreamAa { msaa, fxaa, supersample, id, name }
}

/// The modes the app offers its user, cheapest first.
pub const STREAM_AA_MODES: [StreamAa; 9] = [
    aa(1, false, 1.0, "off", "Off"),
    aa(1, true, 1.0, "fxaa", "FXAA"),
    aa(2, false, 1.0, "msaa2", "MSAA 2x"),
    aa(4, false, 1.0, "msaa4", "MSAA 4x"),
    aa(8, false, 1.0, "msaa8", "MSAA 8x"),
    aa(4, true, 1.0, "msaa4+fxaa", "MSAA 4x + FXAA"),
    aa(1, false, 1.25, "ss1.25", "Supersampled 1.25x"),
    aa(1, false, 1.5, "ss1.5", "Supersampled 1.5x"),
    aa(1, false, 2.0, "ss2", "Supersampled 2x"),
];

/// The default: MSAA 4x.
pub const STREAM_AA_DEFAULT: StreamAa = STREAM_AA_MODES[3];

impl StreamAa {
    pub fn from_id(id: &str) -> Option<StreamAa> {
        STREAM_AA_MODES.into_iter().find(|m| m.id == id)
    }
}

/// What the headset shows around the picture: what the app asks for (`Cx::xr_set_see_through`),
/// or the user's choice of mixed reality (the room shows through the picture's empty parts) or
/// VR (the picture opaque, no passthrough).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamView {
    App,
    Mixed,
    Vr,
}

impl StreamView {
    /// Its name in the settings file.
    pub fn label(self) -> &'static str {
        match self {
            StreamView::App => "app",
            StreamView::Mixed => "mr",
            StreamView::Vr => "vr",
        }
    }

    pub fn from_label(label: &str) -> Option<StreamView> {
        [StreamView::App, StreamView::Mixed, StreamView::Vr].into_iter().find(|v| v.label() == label)
    }
}


/// The lowest eye render scale (percent of the encoded eye size).
pub const MIN_RENDER_SCALE_PCT: u32 = 70;

/// The server's settings for the headset's own per-frame mechanisms (`XrServerMsg::Headset`):
/// the policy lives on the server, the headset executes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadsetSettings {
    /// Fixed foveation, 0 none .. 3 high (2: the user's pick, the edges look the same, the
    /// headset's pass ~20% cheaper).
    pub foveation: u8,
    /// The eye buffers' range on the headset's GPU time (percent of the start size).
    pub eye_min_pct: u8,
    pub eye_max_pct: u8,
    /// Hands and controllers tracked at once.
    pub multimodal: bool,
    /// The room's depth hides the picture.
    pub occlusion: bool,
    /// The poses at least this many display frames past the headset's newest.
    pub ahead: u8,
}

impl HeadsetSettings {
    pub const DEFAULT: HeadsetSettings = HeadsetSettings { foveation: 2, eye_min_pct: 70, eye_max_pct: 100, multimodal: true, occlusion: true, ahead: 4 };
}

