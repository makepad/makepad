//! Program-layer VJ effects. Each kind has two knobs that can ride the beat.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FxId(pub u8);

impl FxId {
    pub const COUNT: u8 = 13;

    pub fn wrap(self, delta: i32) -> FxId {
        let n = Self::COUNT as i32;
        let next = (self.0 as i32 + delta).rem_euclid(n);
        FxId(next as u8)
    }

    pub fn as_f32(self) -> f32 {
        self.0 as f32
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FxInfo {
    pub name: &'static str,
    pub p1: &'static str,
    pub p2: &'static str,
}

pub const FX_INFO: [FxInfo; 13] = [
    FxInfo { name: "OFF", p1: "—", p2: "—" },
    FxInfo { name: "KALEIDO", p1: "SEGS", p2: "SPIN" },
    FxInfo { name: "TUNNEL", p1: "DEPTH", p2: "SPIN" },
    FxInfo { name: "MIRROR", p1: "AXIS", p2: "BLEND" },
    FxInfo { name: "RGB", p1: "SPLIT", p2: "ANGLE" },
    FxInfo { name: "STROBE", p1: "RATE", p2: "DUTY" },
    FxInfo { name: "PIXEL", p1: "SIZE", p2: "SNAP" },
    FxInfo { name: "SWIRL", p1: "TWIST", p2: "RAD" },
    FxInfo { name: "RIPPLE", p1: "AMP", p2: "FREQ" },
    FxInfo { name: "GLITCH", p1: "SLICES", p2: "JITTER" },
    FxInfo { name: "HUE", p1: "SHIFT", p2: "SAT" },
    FxInfo { name: "ZOOM", p1: "IN", p2: "PUNCH" },
    FxInfo { name: "FISH", p1: "BEND", p2: "CROP" },
];

impl FxId {
    pub fn info(self) -> FxInfo {
        FX_INFO[self.0.min(Self::COUNT - 1) as usize]
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FxState {
    pub kind: FxId,
    pub p1: f32,
    pub p2: f32,
    pub beat1: bool,
    pub beat2: bool,
}

impl Default for FxState {
    fn default() -> FxState {
        FxState {
            kind: FxId(0),
            p1: 0.45,
            p2: 0.35,
            beat1: false,
            beat2: true,
        }
    }
}
