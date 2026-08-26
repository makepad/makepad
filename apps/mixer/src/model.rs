//! The mixer state model. The console is the single source of truth: every
//! value here is `Option` and stays `None` until the console has actually
//! told us. The UI shows unknowns as unknowns — no defaults masquerading as
//! state — and other controllers may change anything at any time.

use crate::osc::OscArg;
use crate::safety::{BusN, BusPair, Ch, ChPair, PVal, Param};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Device identity (tolerant `/xinfo` parsing).
// ---------------------------------------------------------------------------

/// What a discovery reply told us. The official doc and community clients
/// disagree about `/xinfo` argument order, so fields are identified by
/// SHAPE (an IP-looking string, a model-looking string, a version-looking
/// string) rather than by position, and the raw arguments are kept for the
/// user to see. The reply's UDP source address is the authoritative IP.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInfo {
    pub ip: Option<String>,
    pub name: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub raw: Vec<String>,
}

fn looks_like_ip(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

fn looks_like_version(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_digit() || c == '.')
        && s.contains('.')
        && !looks_like_ip(s)
}

fn looks_like_model(s: &str) -> bool {
    let u = s.to_ascii_uppercase();
    ["XR18", "XR16", "XR12", "X18", "MR18", "MR12", "X32", "M32"]
        .iter()
        .any(|m| u.contains(m))
}

impl DeviceInfo {
    pub fn from_args(args: &[OscArg]) -> DeviceInfo {
        let mut info = DeviceInfo::default();
        let strings: Vec<String> = args
            .iter()
            .filter_map(|a| match a {
                OscArg::S(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        info.raw = strings.clone();
        // `name=value` extension args first, then shape-classify the rest.
        let mut leftovers: Vec<String> = Vec::new();
        for s in &strings {
            if let Some((k, v)) = s.split_once('=') {
                match k {
                    "device_name" => info.name = Some(v.to_string()),
                    "device_model" => info.model = Some(v.to_string()),
                    "device_version" | "server_version" => {
                        info.firmware.get_or_insert(v.to_string());
                    }
                    _ => {}
                }
            } else {
                leftovers.push(s.clone());
            }
        }
        for s in &leftovers {
            if info.ip.is_none() && looks_like_ip(s) {
                info.ip = Some(s.clone());
            } else if info.model.is_none() && looks_like_model(s) {
                info.model = Some(s.clone());
            }
        }
        for s in &leftovers {
            if info.firmware.is_none()
                && looks_like_version(s)
                && info.model.as_deref() != Some(s.as_str())
            {
                info.firmware = Some(s.clone());
            }
        }
        for s in &leftovers {
            if info.name.is_none()
                && !looks_like_ip(s)
                && !looks_like_version(s)
                && info.model.as_deref() != Some(s.as_str())
            {
                info.name = Some(s.clone());
            }
        }
        info
    }

    pub fn summary(&self) -> String {
        format!(
            "{} \"{}\" fw {}",
            self.model.as_deref().unwrap_or("unknown model"),
            self.name.as_deref().unwrap_or("unnamed"),
            self.firmware.as_deref().unwrap_or("?"),
        )
    }
}

// ---------------------------------------------------------------------------
// Strips.
// ---------------------------------------------------------------------------

/// One strip on the LR-Mix surface. A `paired` strip stands for an
/// odd/even stereo pair and is displayed through the odd member.
///
/// `linked` distinguishes HOW the pair is paired:
///   - true: the console's own stereo link (`/config/chlink`) is ON — the
///     console mirrors the even half, so control talks to the odd only.
///   - false: a VIRTUAL pair — the console reports no link, but the scene
///     is stereo by convention (even channel named "<odd> R", the way this
///     desk is actually run). Control must then drive BOTH channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StripId {
    Ch { base: Ch, paired: bool, linked: bool },
    Bus { base: BusN, paired: bool, linked: bool },
    Main,
}

impl StripId {
    /// Default label ("Ch 1", "Bus 3", "Main 1").
    pub fn label(&self) -> String {
        match self {
            StripId::Ch { base, .. } => format!("Ch {}", base.get()),
            StripId::Bus { base, .. } => format!("Bus {}", base.get()),
            StripId::Main => "Main 1".to_string(),
        }
    }

    /// A virtual pair: gestures must be sent to both halves.
    pub fn is_virtual_pair(&self) -> bool {
        matches!(
            self,
            StripId::Ch { paired: true, linked: false, .. }
                | StripId::Bus { paired: true, linked: false, .. }
        )
    }
}

/// Scribble names often carry the strip's own number ("1 TV" / "2 TV R").
/// The number is not part of the name for comparison purposes.
fn without_number_prefix(name: &str) -> &str {
    let t = name.trim_start();
    let rest = t.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() == t.len() {
        t.trim()
    } else {
        rest.trim_start_matches([' ', '.', ':', '-']).trim()
    }
}

/// The naming convention that marks a manually-run stereo pair: the even
/// half repeats the odd half's name with an "R" tacked on ("TV" / "TV R",
/// and with the desk's own numbering, "1 TV" / "2 TV R").
fn is_right_half(odd_name: &str, even_name: &str) -> bool {
    let o = without_number_prefix(odd_name);
    let e = without_number_prefix(even_name);
    if o.is_empty() || e.is_empty() {
        return false;
    }
    let el = e.to_ascii_lowercase();
    let ol = o.to_ascii_lowercase();
    el == format!("{ol} r") || el == format!("{ol}r") || el == format!("{ol}-r")
}

/// Two halves sitting hard left and hard right are a stereo pair, whatever
/// the console's link switch says — this is how a desk run without chlink
/// marks its pairs, and it is what the meters and faders should follow.
fn is_hard_panned_pair(odd: Option<f32>, even: Option<f32>) -> bool {
    match (odd, even) {
        (Some(o), Some(e)) => o <= 0.02 && e >= 0.98,
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Store.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct MixerModel {
    store: HashMap<Param, PVal>,
    pub device: Option<DeviceInfo>,
}

impl MixerModel {
    pub fn clear(&mut self) {
        self.store.clear();
        self.device = None;
    }

    /// Applies a value reported by the console. Returns true if it changed
    /// (or was previously unknown).
    pub fn apply(&mut self, p: Param, v: PVal) -> bool {
        match self.store.get(&p) {
            Some(old) if *old == v => false,
            _ => {
                self.store.insert(p, v);
                true
            }
        }
    }

    pub fn get(&self, p: Param) -> Option<&PVal> {
        self.store.get(&p)
    }

    pub fn get_f(&self, p: Param) -> Option<f32> {
        match self.store.get(&p) {
            Some(PVal::F(f)) => Some(*f),
            _ => None,
        }
    }

    pub fn get_i(&self, p: Param) -> Option<i32> {
        match self.store.get(&p) {
            Some(PVal::I(i)) => Some(*i),
            _ => None,
        }
    }

    pub fn get_s(&self, p: Param) -> Option<&str> {
        match self.store.get(&p) {
            Some(PVal::S(s)) => Some(s.as_str()),
            _ => None,
        }
    }

    /// Whether every link answer has arrived. The surface no longer waits
    /// for it (channels are paired regardless); the session health line and
    /// the tests use it to tell "console says mono" from "not read yet".
    pub fn links_known(&self) -> bool {
        ChPair::all().all(|p| self.get(Param::ChLink(p)).is_some())
            && BusPair::all().all(|p| self.get(Param::BusLink(p)).is_some())
    }

    /// The strip list the LR-Mix surface shows.
    ///
    /// INPUT CHANNELS ARE ALWAYS PAIRED: this desk is run in stereo pairs,
    /// so 1+2 share one strip (one fader, one mute) whatever the console's
    /// own link switch says. The link state is still read, because it
    /// decides HOW a gesture is sent: a console-LINKED pair mirrors itself,
    /// so control talks to the odd half only; an unlinked ("virtual") pair
    /// must be driven on both halves.
    ///
    /// Buses follow the console: a pair collapses to one strip when the
    /// console links it, when the even half is named "<odd> R", or when the
    /// two halves sit hard L/R (a stereo bus on a desk run without link).
    /// The rest stay mono — that is how bus 5 and 6 stay separate sends.
    /// Main is last.
    pub fn strips(&self) -> Vec<StripId> {
        let mut out = Vec::new();
        for pair in ChPair::all() {
            let odd = Ch::new(pair.get()).unwrap();
            let linked = self.get_i(Param::ChLink(pair)) == Some(1);
            out.push(StripId::Ch { base: odd, paired: true, linked });
        }
        for pair in BusPair::all() {
            let odd = BusN::new(pair.get()).unwrap();
            let even = BusN::new(pair.get() + 1).unwrap();
            if self.get_i(Param::BusLink(pair)) == Some(1) {
                out.push(StripId::Bus { base: odd, paired: true, linked: true });
            } else if is_right_half(
                self.get_s(Param::BusConfigName(odd)).unwrap_or(""),
                self.get_s(Param::BusConfigName(even)).unwrap_or(""),
            ) || is_hard_panned_pair(
                self.get_f(Param::BusMixPan(odd)),
                self.get_f(Param::BusMixPan(even)),
            ) {
                out.push(StripId::Bus { base: odd, paired: true, linked: false });
            } else {
                out.push(StripId::Bus { base: odd, paired: false, linked: false });
                out.push(StripId::Bus { base: even, paired: false, linked: false });
            }
        }
        out.push(StripId::Main);
        out
    }

    /// Scribble name for a strip, falling back to its default label.
    pub fn strip_name(&self, s: StripId) -> String {
        let name = match s {
            StripId::Ch { base, .. } => self.get_s(Param::ChConfigName(base)),
            StripId::Bus { base, .. } => self.get_s(Param::BusConfigName(base)),
            StripId::Main => self.get_s(Param::LrConfigName),
        };
        match name {
            Some(n) if !n.trim().is_empty() => n.to_string(),
            _ => s.label(),
        }
    }

    /// Scribble colour index 0..15 (None until reported).
    pub fn strip_color(&self, s: StripId) -> Option<i32> {
        match s {
            StripId::Ch { base, .. } => self.get_i(Param::ChConfigColor(base)),
            StripId::Bus { base, .. } => self.get_i(Param::BusConfigColor(base)),
            StripId::Main => self.get_i(Param::LrConfigColor),
        }
    }
}

/// The 16-colour scribble palette as linear RGB (index 0..=7 normal,
/// 8..=15 "inverted" — same hues; the UI decides how to use the inversion).
/// The console's colour indexes run 0..=15: 0..=7 are the plain colours
/// (the scribble strip is FILLED with them) and 8..=15 the "inverted" ones
/// (dark strip, coloured text and border). Same colour, opposite fill.
pub fn scribble_inverted(idx: i32) -> bool {
    idx.rem_euclid(16) >= 8
}

pub fn scribble_rgb(idx: i32) -> [f32; 3] {
    match idx.rem_euclid(8) {
        0 => [0.25, 0.25, 0.28], // off/black — lifted so it reads on screen
        1 => [0.95, 0.23, 0.20], // red
        2 => [0.25, 0.85, 0.35], // green
        3 => [0.95, 0.83, 0.20], // yellow
        4 => [0.25, 0.45, 0.95], // blue
        5 => [0.90, 0.30, 0.85], // magenta
        6 => [0.25, 0.80, 0.85], // cyan
        _ => [0.92, 0.92, 0.92], // white
    }
}

// ---------------------------------------------------------------------------
// Meters.
// ---------------------------------------------------------------------------

/// Decoded meter-bank blob: little-endian int16 count prefix, then that
/// many little-endian int16 samples at 1/256 dB.
pub fn decode_meter_blob(payload: &[u8]) -> Option<Vec<f32>> {
    if payload.len() < 4 {
        return None;
    }
    let count = i32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
    let body = &payload[4..];
    if count > 4096 || body.len() < count * 2 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let v = i16::from_le_bytes([body[i * 2], body[i * 2 + 1]]);
        out.push(crate::units::meter_i16_to_db(v));
    }
    Some(out)
}

/// `/meters/1` layout: index of a strip's pre-fader level(s) in the 40-slot
/// bank. Returns (left, right) slot indices; mono strips repeat the slot.
pub fn meters1_slots(s: StripId) -> (usize, usize) {
    match s {
        StripId::Ch { base, paired, .. } => {
            let i = (base.get() - 1) as usize;
            if paired {
                (i, i + 1)
            } else {
                (i, i)
            }
        }
        // 16 ch + aux L/R (16,17) + fx1-4 L/R (18..26) -> bus pre starts at 26.
        StripId::Bus { base, paired, .. } => {
            let i = 26 + (base.get() - 1) as usize;
            if paired {
                (i, i + 1)
            } else {
                (i, i)
            }
        }
        // bus 26..32, fxsend 32..36, main post L/R 36,37.
        StripId::Main => (36, 37),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc::OscArg::S;
    use crate::safety::{PVal, Param};

    #[test]
    fn xinfo_parses_community_order() {
        // community clients: [ip, name, model, firmware]
        let info = DeviceInfo::from_args(&[
            S("192.168.1.71".into()),
            S("MainRack".into()),
            S("XR18".into()),
            S("1.18".into()),
        ]);
        assert_eq!(info.ip.as_deref(), Some("192.168.1.71"));
        assert_eq!(info.name.as_deref(), Some("MainRack"));
        assert_eq!(info.model.as_deref(), Some("XR18"));
        assert_eq!(info.firmware.as_deref(), Some("1.18"));
    }

    #[test]
    fn xinfo_parses_official_order() {
        // official doc: server_version first, name, model, device_version
        let info = DeviceInfo::from_args(&[
            S("2.08".into()),
            S("MainRack".into()),
            S("MR18".into()),
            S("1.22".into()),
        ]);
        assert_eq!(info.model.as_deref(), Some("MR18"));
        assert_eq!(info.name.as_deref(), Some("MainRack"));
        // two version-shaped strings: the first one wins as firmware; both
        // stay visible in raw so nothing is hidden from the user.
        assert_eq!(info.firmware.as_deref(), Some("2.08"));
        assert_eq!(info.raw.len(), 4);
    }

    #[test]
    fn xinfo_parses_name_value_extensions() {
        let info = DeviceInfo::from_args(&[
            S("device_version=1.2".into()),
            S("device_model=XR16".into()),
            S("device_name=Stage".into()),
        ]);
        assert_eq!(info.model.as_deref(), Some("XR16"));
        assert_eq!(info.name.as_deref(), Some("Stage"));
        assert_eq!(info.firmware.as_deref(), Some("1.2"));
    }

    fn linked_model() -> MixerModel {
        let mut m = MixerModel::default();
        for p in ChPair::all() {
            m.apply(Param::ChLink(p), PVal::I(1));
        }
        // buses 1-2 and 3-4 linked, 5-6 mono (the reference session)
        m.apply(Param::BusLink(BusPair::new(1).unwrap()), PVal::I(1));
        m.apply(Param::BusLink(BusPair::new(3).unwrap()), PVal::I(1));
        m.apply(Param::BusLink(BusPair::new(5).unwrap()), PVal::I(0));
        m
    }

    #[test]
    fn strips_follow_link_state() {
        let m = linked_model();
        let strips = m.strips();
        // 8 paired inputs + bus1 + bus3 + bus5 + bus6 + main = 13
        assert_eq!(strips.len(), 13);
        assert_eq!(
            strips[0],
            StripId::Ch { base: Ch::new(1).unwrap(), paired: true, linked: true }
        );
        assert_eq!(
            strips[7],
            StripId::Ch { base: Ch::new(15).unwrap(), paired: true, linked: true }
        );
        assert_eq!(
            strips[8],
            StripId::Bus { base: BusN::new(1).unwrap(), paired: true, linked: true }
        );
        assert_eq!(
            strips[10],
            StripId::Bus { base: BusN::new(5).unwrap(), paired: false, linked: false }
        );
        assert_eq!(
            strips[11],
            StripId::Bus { base: BusN::new(6).unwrap(), paired: false, linked: false }
        );
        assert_eq!(strips[12], StripId::Main);
    }

    #[test]
    fn channels_are_paired_even_when_the_console_says_mono() {
        // The way the real desk is run: console chlink OFF, but the pairs
        // are real. Every channel strip is still a pair — and because the
        // console is not mirroring them, each one is a VIRTUAL pair, so
        // gestures have to be sent to both halves.
        let mut m = MixerModel::default();
        for p in ChPair::all() {
            m.apply(Param::ChLink(p), PVal::I(0));
        }
        for p in BusPair::all() {
            m.apply(Param::BusLink(p), PVal::I(0));
        }
        let ch = |n: u8| Ch::new(n).unwrap();
        let strips = m.strips();
        for (i, odd) in [1u8, 3, 5, 7, 9, 11, 13, 15].iter().enumerate() {
            assert_eq!(
                strips[i],
                StripId::Ch { base: ch(*odd), paired: true, linked: false }
            );
            assert!(strips[i].is_virtual_pair());
        }
        // 8 paired inputs + 6 mono buses + main
        assert_eq!(strips.len(), 8 + 6 + 1);
    }

    #[test]
    fn strips_appear_before_the_link_answers_arrive() {
        // A blank surface while the sweep drains was the old behaviour and
        // it read as "broken app". Channels show immediately; a link answer
        // only changes how they are driven.
        let m = MixerModel::default();
        let strips = m.strips();
        assert_eq!(strips.len(), 8 + 6 + 1);
        assert_eq!(
            strips[0],
            StripId::Ch { base: Ch::new(1).unwrap(), paired: true, linked: false }
        );
        assert!(!m.links_known());
    }

    #[test]
    fn bus_pairs_follow_the_naming_convention() {
        // Buses are NOT force-paired: bus 5 and 6 stay separate sends. A
        // bus pair collapses only when the console links it or the even
        // half is named "<odd> R".
        let mut m = MixerModel::default();
        for p in BusPair::all() {
            m.apply(Param::BusLink(p), PVal::I(0));
        }
        let bus = |n: u8| BusN::new(n).unwrap();
        m.apply(Param::BusConfigName(bus(1)), PVal::S("Speaker".into()));
        m.apply(Param::BusConfigName(bus(2)), PVal::S("Speaker R".into()));
        let strips = m.strips();
        assert_eq!(
            strips[8],
            StripId::Bus { base: bus(1), paired: true, linked: false }
        );
        assert_eq!(
            strips[9],
            StripId::Bus { base: bus(3), paired: false, linked: false }
        );
        // 8 paired inputs + bus1(pair) + bus3..6 mono + main
        assert_eq!(strips.len(), 8 + 5 + 1);
    }

    #[test]
    fn strip_names_fall_back_to_labels() {
        let mut m = linked_model();
        let ch1 = StripId::Ch { base: Ch::new(1).unwrap(), paired: true, linked: true };
        assert_eq!(m.strip_name(ch1), "Ch 1");
        m.apply(Param::ChConfigName(Ch::new(1).unwrap()), PVal::S("TV".into()));
        assert_eq!(m.strip_name(ch1), "TV");
    }

    #[test]
    fn meter_blob_roundtrip() {
        // LE count + LE shorts, silence marker included
        let mut payload = vec![3, 0, 0, 0];
        for v in [0i16, -25600, i16::MIN] {
            payload.extend_from_slice(&v.to_le_bytes());
        }
        let d = decode_meter_blob(&payload).unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(d[0], 0.0);
        assert_eq!(d[1], -100.0);
        assert!(d[2].is_infinite());
        assert!(decode_meter_blob(&[1, 0]).is_none());
    }
}
