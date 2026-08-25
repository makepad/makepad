//! The safety layer. Read this before touching any network code.
//!
//! A digital mixer's OSC dialect makes a SET out of any message that carries
//! an argument, and one address family (`/headamp/NN/phantom`) switches 48 V
//! phantom power that can destroy connected equipment. Snapshot/preset
//! recalls change phantom states as a side effect, and `/-action/initall`
//! wipes the console. This module makes those messages *unconstructable*:
//!
//! 1. **Compile-time whitelist.** [`Param`] is a closed enum of the LR-Mix
//!    surface parameters. [`SafeMsg`] has private fields and can only be
//!    built by `Param::get` / `Param::set` (which validate argument shape
//!    and range — no coercion) and by the four named subscription
//!    constructors (`xinfo`, `status`, `xremote`, `meters_subscribe`).
//!    There is no "raw address" constructor. Phantom power, snapshots,
//!    scenes, presets, routing, preferences and actions are not variants,
//!    so no code path in this crate can express them.
//!
//! 2. **Runtime deny list at the socket.** [`GuardedSocket::transmit`] is
//!    the ONLY place in the client that calls `UdpSocket::send_to`. It
//!    re-parses the address out of the encoded bytes it is about to write
//!    and refuses anything containing a deny-listed term — independently of
//!    the whitelist, so even a hypothetical bug that smuggled bytes into a
//!    `SafeMsg` is stopped at the last possible point.
//!
//! 3. **Loopback jail for test runs.** When constructed with
//!    `loopback_only`, the same chokepoint refuses any destination that is
//!    not a loopback address — the mode every automated run of this app
//!    uses. Real-network operation only happens when a human launches the
//!    app without `--fake` and explicitly targets a mixer.

use crate::osc::{peek_address, peek_has_args, OscArg, OscMsg};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Deny list — belt and braces, checked at the socket write.
// ---------------------------------------------------------------------------

/// Any outgoing address containing one of these (case-insensitive) is
/// refused at the transmit chokepoint. The first ten terms are a standing
/// requirement; the tail adds the routing/init families from the protocol
/// notes' danger table.
pub const DENY_TERMS: &[&str] = &[
    "phantom", "-snap", "-action", "-prefs", "-stat", "-libs", "-show", "load", "save", "-usb",
    "/snap", "routing", "initall", "insrc", "rtnsrc", "showdump", "formatsubscribe",
    "batchsubscribe", "/subscribe", "/renew",
];

/// Returns the deny-list term an address matches, if any.
pub fn deny_term(addr: &str) -> Option<&'static str> {
    let lower = addr.to_ascii_lowercase();
    DENY_TERMS.iter().find(|t| lower.contains(*t)).copied()
}

// ---------------------------------------------------------------------------
// Validated index newtypes.
// ---------------------------------------------------------------------------

macro_rules! bounded_u8 {
    ($name:ident, $min:literal ..= $max:literal, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u8);
        impl $name {
            pub fn new(v: u8) -> Option<Self> {
                if ($min..=$max).contains(&v) {
                    Some(Self(v))
                } else {
                    None
                }
            }
            pub fn get(self) -> u8 {
                self.0
            }
            pub fn all() -> impl Iterator<Item = Self> {
                ($min..=$max).map(Self)
            }
        }
    };
}

bounded_u8!(Ch, 1..=16, "Input channel 1..=16.");
bounded_u8!(BusN, 1..=6, "Mix bus 1..=6 (unpadded on the wire).");
bounded_u8!(SendBus, 1..=6, "Bus-send slot 1..=6 (zero-padded on the wire).");
bounded_u8!(EqBand, 1..=4, "Input-channel EQ band 1..=4.");
bounded_u8!(EqBand6, 1..=6, "Bus/main EQ band 1..=6.");

/// Stereo-link pair, identified by its odd base channel (1,3,..,15).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ChPair(u8);
impl ChPair {
    pub fn new(odd: u8) -> Option<Self> {
        if odd % 2 == 1 && (1..=15).contains(&odd) {
            Some(Self(odd))
        } else {
            None
        }
    }
    pub fn get(self) -> u8 {
        self.0
    }
    pub fn all() -> impl Iterator<Item = Self> {
        (1..=15).step_by(2).map(Self)
    }
}

/// Bus-link pair, identified by its odd base bus (1,3,5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BusPair(u8);
impl BusPair {
    pub fn new(odd: u8) -> Option<Self> {
        if odd % 2 == 1 && (1..=5).contains(&odd) {
            Some(Self(odd))
        } else {
            None
        }
    }
    pub fn get(self) -> u8 {
        self.0
    }
    pub fn all() -> impl Iterator<Item = Self> {
        (1..=5).step_by(2).map(Self)
    }
}

// ---------------------------------------------------------------------------
// Leaves.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EqLeaf {
    Type,
    F,
    G,
    Q,
}
impl EqLeaf {
    pub const ALL: [EqLeaf; 4] = [EqLeaf::Type, EqLeaf::F, EqLeaf::G, EqLeaf::Q];
    fn seg(self) -> &'static str {
        match self {
            EqLeaf::Type => "type",
            EqLeaf::F => "f",
            EqLeaf::G => "g",
            EqLeaf::Q => "q",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GateLeaf {
    On,
    Mode,
    Thr,
    Range,
    Attack,
    Hold,
    Release,
}
impl GateLeaf {
    pub const ALL: [GateLeaf; 7] = [
        GateLeaf::On,
        GateLeaf::Mode,
        GateLeaf::Thr,
        GateLeaf::Range,
        GateLeaf::Attack,
        GateLeaf::Hold,
        GateLeaf::Release,
    ];
    fn seg(self) -> &'static str {
        match self {
            GateLeaf::On => "on",
            GateLeaf::Mode => "mode",
            GateLeaf::Thr => "thr",
            GateLeaf::Range => "range",
            GateLeaf::Attack => "attack",
            GateLeaf::Hold => "hold",
            GateLeaf::Release => "release",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DynLeaf {
    On,
    Mode,
    Det,
    Env,
    Thr,
    Ratio,
    Knee,
    Mgain,
    Attack,
    Hold,
    Release,
    Mix,
}
impl DynLeaf {
    pub const ALL: [DynLeaf; 12] = [
        DynLeaf::On,
        DynLeaf::Mode,
        DynLeaf::Det,
        DynLeaf::Env,
        DynLeaf::Thr,
        DynLeaf::Ratio,
        DynLeaf::Knee,
        DynLeaf::Mgain,
        DynLeaf::Attack,
        DynLeaf::Hold,
        DynLeaf::Release,
        DynLeaf::Mix,
    ];
    fn seg(self) -> &'static str {
        match self {
            DynLeaf::On => "on",
            DynLeaf::Mode => "mode",
            DynLeaf::Det => "det",
            DynLeaf::Env => "env",
            DynLeaf::Thr => "thr",
            DynLeaf::Ratio => "ratio",
            DynLeaf::Knee => "knee",
            DynLeaf::Mgain => "mgain",
            DynLeaf::Attack => "attack",
            DynLeaf::Hold => "hold",
            DynLeaf::Release => "release",
            DynLeaf::Mix => "mix",
        }
    }
}

// ---------------------------------------------------------------------------
// The whitelist.
// ---------------------------------------------------------------------------

/// Every parameter this application can read or write. This is the entire
/// vocabulary — there is deliberately no variant for phantom power,
/// snapshots, scenes, presets, routing, preferences, actions, libraries,
/// shows, or USB. A surface layout (splash) can only name parameters that
/// exist here, so a generated layout cannot widen the app's reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Param {
    ChMixFader(Ch),
    ChMixOn(Ch),
    ChMixPan(Ch),
    ChMixLr(Ch),
    ChSendLevel(Ch, SendBus),
    ChConfigName(Ch),
    ChConfigColor(Ch),
    ChEqOn(Ch),
    ChEq(Ch, EqBand, EqLeaf),
    ChGate(Ch, GateLeaf),
    ChDyn(Ch, DynLeaf),
    HeadampGain(Ch),
    ChLink(ChPair),
    BusLink(BusPair),
    BusMixFader(BusN),
    BusMixOn(BusN),
    BusMixPan(BusN),
    BusConfigName(BusN),
    BusConfigColor(BusN),
    BusEqOn(BusN),
    BusEq(BusN, EqBand6, EqLeaf),
    BusDyn(BusN, DynLeaf),
    LrMixFader,
    LrMixOn,
    LrMixPan,
    LrConfigName,
    LrConfigColor,
    LrEqOn,
    LrEq(EqBand6, EqLeaf),
    LrDyn(DynLeaf),
}

/// Argument shape a parameter accepts. Wrong shape is an error, never a
/// coercion — an int where a float belongs is refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ValueSpec {
    /// float32, finite, 0.0..=1.0 (the wire range for every continuous
    /// parameter; engineering units are a client-side map).
    Float01,
    /// int32 in 0..=max.
    Int { max: i32 },
    /// ASCII string, at most 12 chars, no control characters.
    Name,
}

/// A runtime value headed for (or coming from) the wire.
#[derive(Clone, Debug, PartialEq)]
pub enum PVal {
    F(f32),
    I(i32),
    S(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum SafetyError {
    WrongType { param: Param, expect: ValueSpec },
    OutOfRange { param: Param },
    BadName { reason: &'static str },
}

impl Param {
    /// Renders the OSC address. This function and the subscription
    /// constructors below are the only address factories in the crate.
    pub fn addr(&self) -> String {
        match *self {
            Param::ChMixFader(c) => format!("/ch/{:02}/mix/fader", c.get()),
            Param::ChMixOn(c) => format!("/ch/{:02}/mix/on", c.get()),
            Param::ChMixPan(c) => format!("/ch/{:02}/mix/pan", c.get()),
            Param::ChMixLr(c) => format!("/ch/{:02}/mix/lr", c.get()),
            Param::ChSendLevel(c, b) => format!("/ch/{:02}/mix/{:02}/level", c.get(), b.get()),
            Param::ChConfigName(c) => format!("/ch/{:02}/config/name", c.get()),
            Param::ChConfigColor(c) => format!("/ch/{:02}/config/color", c.get()),
            Param::ChEqOn(c) => format!("/ch/{:02}/eq/on", c.get()),
            Param::ChEq(c, b, l) => format!("/ch/{:02}/eq/{}/{}", c.get(), b.get(), l.seg()),
            Param::ChGate(c, l) => format!("/ch/{:02}/gate/{}", c.get(), l.seg()),
            Param::ChDyn(c, l) => format!("/ch/{:02}/dyn/{}", c.get(), l.seg()),
            Param::HeadampGain(c) => format!("/headamp/{:02}/gain", c.get()),
            Param::ChLink(p) => format!("/config/chlink/{}-{}", p.get(), p.get() + 1),
            Param::BusLink(p) => format!("/config/buslink/{}-{}", p.get(), p.get() + 1),
            Param::BusMixFader(b) => format!("/bus/{}/mix/fader", b.get()),
            Param::BusMixOn(b) => format!("/bus/{}/mix/on", b.get()),
            Param::BusMixPan(b) => format!("/bus/{}/mix/pan", b.get()),
            Param::BusConfigName(b) => format!("/bus/{}/config/name", b.get()),
            Param::BusConfigColor(b) => format!("/bus/{}/config/color", b.get()),
            Param::BusEqOn(b) => format!("/bus/{}/eq/on", b.get()),
            Param::BusEq(b, band, l) => format!("/bus/{}/eq/{}/{}", b.get(), band.get(), l.seg()),
            Param::BusDyn(b, l) => format!("/bus/{}/dyn/{}", b.get(), l.seg()),
            Param::LrMixFader => "/lr/mix/fader".to_string(),
            Param::LrMixOn => "/lr/mix/on".to_string(),
            Param::LrMixPan => "/lr/mix/pan".to_string(),
            Param::LrConfigName => "/lr/config/name".to_string(),
            Param::LrConfigColor => "/lr/config/color".to_string(),
            Param::LrEqOn => "/lr/eq/on".to_string(),
            Param::LrEq(band, l) => format!("/lr/eq/{}/{}", band.get(), l.seg()),
            Param::LrDyn(l) => format!("/lr/dyn/{}", l.seg()),
        }
    }

    pub fn spec(&self) -> ValueSpec {
        use Param::*;
        match *self {
            ChMixOn(_) | BusMixOn(_) | LrMixOn | ChMixLr(_) | ChEqOn(_) | BusEqOn(_) | LrEqOn
            | ChLink(_) | BusLink(_) => ValueSpec::Int { max: 1 },
            ChConfigColor(_) | BusConfigColor(_) | LrConfigColor => ValueSpec::Int { max: 15 },
            ChConfigName(_) | BusConfigName(_) | LrConfigName => ValueSpec::Name,
            ChEq(_, _, l) | BusEq(_, _, l) | LrEq(_, l) => match l {
                EqLeaf::Type => ValueSpec::Int { max: 5 },
                _ => ValueSpec::Float01,
            },
            ChGate(_, l) => match l {
                GateLeaf::On => ValueSpec::Int { max: 1 },
                GateLeaf::Mode => ValueSpec::Int { max: 4 },
                _ => ValueSpec::Float01,
            },
            ChDyn(_, l) | BusDyn(_, l) | LrDyn(l) => match l {
                DynLeaf::On | DynLeaf::Mode | DynLeaf::Det | DynLeaf::Env => {
                    ValueSpec::Int { max: 1 }
                }
                DynLeaf::Ratio => ValueSpec::Int { max: 11 },
                _ => ValueSpec::Float01,
            },
            _ => ValueSpec::Float01,
        }
    }

    /// A GET: the address with no arguments. Always safe — the console
    /// answers with the current value.
    pub fn get(&self) -> SafeMsg {
        SafeMsg::from_parts(self.addr(), Vec::new())
    }

    /// A SET. The value must match the parameter's [`ValueSpec`] exactly;
    /// a wrong type or out-of-range value is an error, never coerced.
    pub fn set(&self, v: &PVal) -> Result<SafeMsg, SafetyError> {
        let arg = match (self.spec(), v) {
            (ValueSpec::Float01, PVal::F(f)) => {
                if !f.is_finite() || !(0.0..=1.0).contains(f) {
                    return Err(SafetyError::OutOfRange { param: *self });
                }
                OscArg::F(*f)
            }
            (ValueSpec::Int { max }, PVal::I(i)) => {
                if !(0..=max).contains(i) {
                    return Err(SafetyError::OutOfRange { param: *self });
                }
                OscArg::I(*i)
            }
            (ValueSpec::Name, PVal::S(s)) => {
                if s.len() > 12 {
                    return Err(SafetyError::BadName { reason: "longer than 12 chars" });
                }
                if !s.chars().all(|c| c.is_ascii() && !c.is_ascii_control()) {
                    return Err(SafetyError::BadName { reason: "non-ASCII or control char" });
                }
                OscArg::S(s.clone())
            }
            (spec, _) => return Err(SafetyError::WrongType { param: *self, expect: spec }),
        };
        Ok(SafeMsg::from_parts(self.addr(), vec![arg]))
    }

    /// Parses a whitelisted incoming address back into a `Param`. Anything
    /// else — including every dangerous family — returns `None` and is
    /// ignored by the model.
    pub fn parse(addr: &str) -> Option<Param> {
        let seg: Vec<&str> = addr.strip_prefix('/')?.split('/').collect();
        match seg.as_slice() {
            ["ch", ch, rest @ ..] => {
                let c = Ch::new(ch.parse::<u8>().ok().filter(|_| ch.len() == 2)?)?;
                match rest {
                    ["mix", "fader"] => Some(Param::ChMixFader(c)),
                    ["mix", "on"] => Some(Param::ChMixOn(c)),
                    ["mix", "pan"] => Some(Param::ChMixPan(c)),
                    ["mix", "lr"] => Some(Param::ChMixLr(c)),
                    ["mix", send, "level"] => {
                        let b = SendBus::new(send.parse::<u8>().ok().filter(|_| send.len() == 2)?)?;
                        Some(Param::ChSendLevel(c, b))
                    }
                    ["config", "name"] => Some(Param::ChConfigName(c)),
                    ["config", "color"] => Some(Param::ChConfigColor(c)),
                    ["eq", "on"] => Some(Param::ChEqOn(c)),
                    ["eq", band, leaf] => {
                        let b = EqBand::new(band.parse().ok()?)?;
                        Some(Param::ChEq(c, b, parse_eq_leaf(leaf)?))
                    }
                    ["gate", leaf] => Some(Param::ChGate(c, parse_gate_leaf(leaf)?)),
                    ["dyn", leaf] => Some(Param::ChDyn(c, parse_dyn_leaf(leaf)?)),
                    _ => None,
                }
            }
            ["headamp", ha, "gain"] => {
                let c = Ch::new(ha.parse::<u8>().ok().filter(|_| ha.len() == 2)?)?;
                Some(Param::HeadampGain(c))
            }
            ["config", "chlink", pair] => {
                let odd: u8 = pair.split('-').next()?.parse().ok()?;
                Some(Param::ChLink(ChPair::new(odd)?))
            }
            ["config", "buslink", pair] => {
                let odd: u8 = pair.split('-').next()?.parse().ok()?;
                Some(Param::BusLink(BusPair::new(odd)?))
            }
            ["bus", bus, rest @ ..] => {
                let b = BusN::new(bus.parse().ok()?)?;
                match rest {
                    ["mix", "fader"] => Some(Param::BusMixFader(b)),
                    ["mix", "on"] => Some(Param::BusMixOn(b)),
                    ["mix", "pan"] => Some(Param::BusMixPan(b)),
                    ["config", "name"] => Some(Param::BusConfigName(b)),
                    ["config", "color"] => Some(Param::BusConfigColor(b)),
                    ["eq", "on"] => Some(Param::BusEqOn(b)),
                    ["eq", band, leaf] => {
                        let band = EqBand6::new(band.parse().ok()?)?;
                        Some(Param::BusEq(b, band, parse_eq_leaf(leaf)?))
                    }
                    ["dyn", leaf] => Some(Param::BusDyn(b, parse_dyn_leaf(leaf)?)),
                    _ => None,
                }
            }
            ["lr", rest @ ..] => match rest {
                ["mix", "fader"] => Some(Param::LrMixFader),
                ["mix", "on"] => Some(Param::LrMixOn),
                ["mix", "pan"] => Some(Param::LrMixPan),
                ["config", "name"] => Some(Param::LrConfigName),
                ["config", "color"] => Some(Param::LrConfigColor),
                ["eq", "on"] => Some(Param::LrEqOn),
                ["eq", band, leaf] => {
                    let band = EqBand6::new(band.parse().ok()?)?;
                    Some(Param::LrEq(band, parse_eq_leaf(leaf)?))
                }
                ["dyn", leaf] => Some(Param::LrDyn(parse_dyn_leaf(leaf)?)),
                _ => None,
            },
            _ => None,
        }
    }

    /// The full GET sweep a fresh connection performs: link state, then every
    /// surface parameter for channels, buses and the main bus. Order matters
    /// a little — links and names first so the surface can assemble early.
    pub fn surface_sweep() -> Vec<Param> {
        let mut v = Vec::with_capacity(900);
        v.extend(ChPair::all().map(Param::ChLink));
        v.extend(BusPair::all().map(Param::BusLink));
        for c in Ch::all() {
            v.push(Param::ChConfigName(c));
            v.push(Param::ChConfigColor(c));
        }
        for b in BusN::all() {
            v.push(Param::BusConfigName(b));
            v.push(Param::BusConfigColor(b));
        }
        v.push(Param::LrConfigName);
        v.push(Param::LrConfigColor);
        for c in Ch::all() {
            v.push(Param::ChMixFader(c));
            v.push(Param::ChMixOn(c));
            v.push(Param::ChMixPan(c));
            v.push(Param::ChMixLr(c));
            v.push(Param::HeadampGain(c));
            v.push(Param::ChEqOn(c));
            for band in EqBand::all() {
                for l in EqLeaf::ALL {
                    v.push(Param::ChEq(c, band, l));
                }
            }
            for l in GateLeaf::ALL {
                v.push(Param::ChGate(c, l));
            }
            for l in DynLeaf::ALL {
                v.push(Param::ChDyn(c, l));
            }
        }
        for b in BusN::all() {
            v.push(Param::BusMixFader(b));
            v.push(Param::BusMixOn(b));
            v.push(Param::BusMixPan(b));
            v.push(Param::BusEqOn(b));
            for band in EqBand6::all() {
                for l in EqLeaf::ALL {
                    v.push(Param::BusEq(b, band, l));
                }
            }
            for l in DynLeaf::ALL {
                v.push(Param::BusDyn(b, l));
            }
        }
        v.push(Param::LrMixFader);
        v.push(Param::LrMixOn);
        v.push(Param::LrMixPan);
        v.push(Param::LrEqOn);
        for band in EqBand6::all() {
            for l in EqLeaf::ALL {
                v.push(Param::LrEq(band, l));
            }
        }
        for l in DynLeaf::ALL {
            v.push(Param::LrDyn(l));
        }
        v
    }

    /// EVERY parameter address this crate can construct — the surface sweep
    /// plus the families the current UI doesn't read (bus send levels).
    /// The wire audit and the collision tests enumerate THIS, so the audit
    /// is generated from the whitelist itself and cannot drift from it.
    pub fn all_constructable() -> Vec<Param> {
        let mut v = Param::surface_sweep();
        for c in Ch::all() {
            for b in SendBus::all() {
                v.push(Param::ChSendLevel(c, b));
            }
        }
        v
    }
}

fn parse_eq_leaf(s: &str) -> Option<EqLeaf> {
    EqLeaf::ALL.into_iter().find(|l| l.seg() == s)
}
fn parse_gate_leaf(s: &str) -> Option<GateLeaf> {
    GateLeaf::ALL.into_iter().find(|l| l.seg() == s)
}
fn parse_dyn_leaf(s: &str) -> Option<DynLeaf> {
    DynLeaf::ALL.into_iter().find(|l| l.seg() == s)
}

// ---------------------------------------------------------------------------
// SafeMsg — the only thing the socket will transmit.
// ---------------------------------------------------------------------------

/// An outgoing message whose address came from the whitelist. Fields are
/// private: outside this module there is no way to make one from a raw
/// address string.
#[derive(Clone, Debug)]
pub struct SafeMsg {
    addr: String,
    bytes: Vec<u8>,
    human: String,
}

impl SafeMsg {
    fn from_parts(addr: String, args: Vec<OscArg>) -> SafeMsg {
        let human = if args.is_empty() {
            format!("{} (get)", addr)
        } else {
            format!("{} {:?}", addr, args)
        };
        let bytes = OscMsg { addr: addr.clone(), args }.encode();
        SafeMsg { addr, bytes, human }
    }

    /// `/xinfo` — identity query. Read-only; used for discovery and to head
    /// a connection. Sent only on explicit user action.
    pub fn xinfo() -> SafeMsg {
        SafeMsg::from_parts("/xinfo".to_string(), Vec::new())
    }

    /// `/status` — server status query. Read-only.
    pub fn status() -> SafeMsg {
        SafeMsg::from_parts("/status".to_string(), Vec::new())
    }

    /// `/xremote` — subscribe this ip:port to parameter pushes for 10 s.
    pub fn xremote() -> SafeMsg {
        SafeMsg::from_parts("/xremote".to_string(), Vec::new())
    }

    /// `/meters ,s "/meters/N"` — subscribe to a meter bank blob stream
    /// (~50 ms cadence, self-expires after 10 s). Only the two banks the
    /// surface uses are constructable.
    pub fn meters_subscribe(bank: MeterBank) -> SafeMsg {
        let id = match bank {
            MeterBank::Channels => "/meters/1",
            MeterBank::Dynamics => "/meters/6",
        };
        SafeMsg::from_parts("/meters".to_string(), vec![OscArg::S(id.to_string())])
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub fn human(&self) -> &str {
        &self.human
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeterBank {
    /// `/meters/1`: 40 int16 — 16 ch pre, aux L/R, fx 1-4 L/R, bus 1-6,
    /// fxsend 1-4, main post L/R, monitor L/R.
    Channels,
    /// `/meters/6`: 39 int16 — 16 gate GR, 16 ch dyn GR, 6 bus dyn GR, LR dyn GR.
    Dynamics,
}

// ---------------------------------------------------------------------------
// The guarded socket — the single transmit chokepoint.
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Refused {
    /// Address matched the deny list. Must never happen — a `SafeMsg`
    /// cannot carry such an address — but the gate does not trust that.
    Denied { addr: String, term: &'static str },
    /// Destination outside loopback while running in the loopback jail.
    NotLoopback(SocketAddr),
    /// Bytes did not parse as an OSC message (refused rather than sent).
    BadPacket,
    /// Read-only mode: the packet carries an argument (= a SET in this
    /// dialect) and is not the /meters subscribe. Stage-1 sessions refuse
    /// every such packet at the socket, whatever produced it.
    ReadOnly { addr: String },
    Io(std::io::Error),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Refused::Denied { addr, term } => {
                write!(f, "REFUSED deny-listed address {:?} (term {:?})", addr, term)
            }
            Refused::NotLoopback(d) => write!(f, "REFUSED non-loopback destination {}", d),
            Refused::BadPacket => write!(f, "REFUSED unparseable packet"),
            Refused::ReadOnly { addr } => {
                write!(f, "REFUSED write to {:?} in read-only mode", addr)
            }
            Refused::Io(e) => write!(f, "socket error: {}", e),
        }
    }
}

pub struct GuardedSocket {
    sock: UdpSocket,
    loopback_only: bool,
    wire_log: bool,
    /// Stage-1 gate, ON at bind. While set, any outgoing packet that
    /// carries an argument — i.e. any SET — is refused at the socket; only
    /// bare-address queries and the "/meters" subscribe pass. Flipped off
    /// exclusively by the user's CONTROL toggle.
    read_only: AtomicBool,
}

impl GuardedSocket {
    /// Binds to an OS-assigned source port (never a fixed one, so several
    /// controllers can run side by side). `loopback_only` is the jail every
    /// automated run uses: it binds to 127.0.0.1 AND refuses non-loopback
    /// destinations at transmit time.
    pub fn bind(loopback_only: bool) -> std::io::Result<GuardedSocket> {
        let sock = if loopback_only {
            UdpSocket::bind("127.0.0.1:0")?
        } else {
            UdpSocket::bind("0.0.0.0:0")?
        };
        sock.set_read_timeout(Some(std::time::Duration::from_millis(15)))?;
        if !loopback_only {
            // Needed for directed-broadcast discovery. Harmless otherwise.
            let _ = sock.set_broadcast(true);
        }
        Ok(GuardedSocket {
            sock,
            loopback_only,
            wire_log: true,
            read_only: AtomicBool::new(true),
        })
    }

    /// Flips the stage-1 read-only gate. Only the UI's explicit CONTROL
    /// toggle calls this with `false`.
    pub fn set_read_only(&self, on: bool) {
        self.read_only.store(on, Ordering::SeqCst);
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    pub fn set_wire_log(&mut self, on: bool) {
        self.wire_log = on;
    }

    pub fn loopback_only(&self) -> bool {
        self.loopback_only
    }

    /// Sends a whitelisted message. Everything funnels into
    /// [`Self::transmit`].
    pub fn send(&self, dest: SocketAddr, msg: &SafeMsg) -> Result<(), Refused> {
        self.transmit(dest, &msg.bytes, msg.human())
    }

    /// THE transmit chokepoint — the only `send_to` in the client. The
    /// address is re-parsed from the encoded bytes so the deny check sees
    /// exactly what would hit the wire.
    fn transmit(&self, dest: SocketAddr, bytes: &[u8], human: &str) -> Result<(), Refused> {
        let addr = peek_address(bytes).map_err(|_| Refused::BadPacket)?;
        if let Some(term) = deny_term(&addr) {
            let refusal = Refused::Denied { addr, term };
            eprintln!("[mixer-wire] {}", refusal);
            return Err(refusal);
        }
        if self.read_only.load(Ordering::SeqCst)
            && addr != "/meters"
            && peek_has_args(bytes).map_err(|_| Refused::BadPacket)?
        {
            let refusal = Refused::ReadOnly { addr };
            eprintln!("[mixer-wire] {}", refusal);
            return Err(refusal);
        }
        if self.loopback_only && !dest.ip().is_loopback() {
            let refusal = Refused::NotLoopback(dest);
            eprintln!("[mixer-wire] {}", refusal);
            return Err(refusal);
        }
        if self.wire_log {
            println!("[mixer-wire] TX -> {} : {}", dest, human);
        }
        self.sock.send_to(bytes, dest).map_err(Refused::Io)?;
        Ok(())
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        self.sock.recv_from(buf)
    }

    /// Test-only hostile injection: builds a packet for an ARBITRARY address
    /// and pushes it at the same transmit tail a real send uses. This exists
    /// to prove the deny gate holds even if the whitelist were somehow
    /// bypassed. Not compiled into the application binary.
    #[cfg(test)]
    pub fn hostile_transmit(&self, dest: SocketAddr, msg: &OscMsg) -> Result<(), Refused> {
        self.transmit(dest, &msg.encode(), "HOSTILE")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every constructable whitelisted address must stay clear of the deny
    /// list — otherwise the two layers would fight.
    #[test]
    fn whitelist_never_collides_with_deny_list() {
        for p in Param::surface_sweep() {
            let a = p.addr();
            assert!(
                deny_term(&a).is_none(),
                "whitelisted {:?} matches deny term {:?}",
                a,
                deny_term(&a)
            );
        }
        for m in [
            SafeMsg::xinfo(),
            SafeMsg::status(),
            SafeMsg::xremote(),
            SafeMsg::meters_subscribe(MeterBank::Channels),
            SafeMsg::meters_subscribe(MeterBank::Dynamics),
        ] {
            assert!(deny_term(m.addr()).is_none());
        }
    }

    #[test]
    fn address_rendering_matches_protocol() {
        assert_eq!(
            Param::ChMixFader(Ch::new(1).unwrap()).addr(),
            "/ch/01/mix/fader"
        );
        assert_eq!(
            Param::ChSendLevel(Ch::new(16).unwrap(), SendBus::new(3).unwrap()).addr(),
            "/ch/16/mix/03/level"
        );
        assert_eq!(Param::BusMixFader(BusN::new(6).unwrap()).addr(), "/bus/6/mix/fader");
        assert_eq!(Param::LrMixFader.addr(), "/lr/mix/fader");
        assert_eq!(
            Param::HeadampGain(Ch::new(9).unwrap()).addr(),
            "/headamp/09/gain"
        );
        assert_eq!(
            Param::ChLink(ChPair::new(15).unwrap()).addr(),
            "/config/chlink/15-16"
        );
        assert_eq!(
            Param::ChEq(Ch::new(2).unwrap(), EqBand::new(4).unwrap(), EqLeaf::Q).addr(),
            "/ch/02/eq/4/q"
        );
    }

    #[test]
    fn parse_is_inverse_of_addr() {
        for p in Param::surface_sweep() {
            assert_eq!(Param::parse(&p.addr()), Some(p), "addr {}", p.addr());
        }
    }

    #[test]
    fn parse_rejects_dangerous_and_unknown() {
        for a in [
            "/headamp/01/phantom",
            "/-snap/load",
            "/-action/initall",
            "/-prefs/clockrate",
            "/routing/main/01",
            "/ch/01/preamp/hpon", // real but not whitelisted in v1
            "/ch/17/mix/fader",   // out-of-range index
            "/bus/7/mix/fader",
            "/ch/1/mix/fader", // unpadded channel is not the wire form
        ] {
            assert_eq!(Param::parse(a), None, "should not parse {}", a);
        }
    }

    #[test]
    fn set_rejects_wrong_shapes_instead_of_coercing() {
        let fader = Param::ChMixFader(Ch::new(1).unwrap());
        // int where float belongs: refused
        assert!(matches!(
            fader.set(&PVal::I(1)),
            Err(SafetyError::WrongType { .. })
        ));
        // out-of-range float: refused
        assert!(matches!(
            fader.set(&PVal::F(1.5)),
            Err(SafetyError::OutOfRange { .. })
        ));
        assert!(matches!(
            fader.set(&PVal::F(f32::NAN)),
            Err(SafetyError::OutOfRange { .. })
        ));
        assert!(matches!(
            fader.set(&PVal::F(-0.1)),
            Err(SafetyError::OutOfRange { .. })
        ));
        let on = Param::ChMixOn(Ch::new(1).unwrap());
        // float where int belongs: refused, NOT rounded
        assert!(matches!(
            on.set(&PVal::F(1.0)),
            Err(SafetyError::WrongType { .. })
        ));
        assert!(matches!(
            on.set(&PVal::I(2)),
            Err(SafetyError::OutOfRange { .. })
        ));
        let color = Param::ChConfigColor(Ch::new(1).unwrap());
        assert!(matches!(
            color.set(&PVal::I(16)),
            Err(SafetyError::OutOfRange { .. })
        ));
        let name = Param::ChConfigName(Ch::new(1).unwrap());
        assert!(name.set(&PVal::S("a-name-that-is-too-long".into())).is_err());
        assert!(name.set(&PVal::S("Drüms".into())).is_err());
        assert!(name.set(&PVal::S("Drums".into())).is_ok());
        // valid sets pass
        assert!(fader.set(&PVal::F(0.75)).is_ok());
        assert!(on.set(&PVal::I(0)).is_ok());
    }

    #[test]
    fn deny_list_terms_hit_expected_families() {
        for (addr, _why) in [
            ("/headamp/01/phantom", "48V"),
            ("/-snap/load", "snapshot recall"),
            ("/snap/save", "snapshot save"),
            ("/-action/initall", "console wipe"),
            ("/-action/clearsolo", "action"),
            ("/-prefs/clockrate", "prefs"),
            ("/-stat/tape/state", "transport"),
            ("/-stat/solosw/01", "solo"),
            ("/-libs/ch/01", "preset library"),
            ("/-show/prepos", "show"),
            ("/load", "x32 load"),
            ("/save", "x32 save"),
            ("/-usb/path", "usb"),
            ("/routing/main/01", "repatch"),
            ("/config/routing/IN", "x32 repatch"),
            ("/ch/01/config/insrc", "input repatch"),
        ] {
            assert!(deny_term(addr).is_some(), "deny list must match {}", addr);
        }
        // and the whole surface vocabulary stays clean
        assert!(deny_term("/ch/01/mix/fader").is_none());
        assert!(deny_term("/status").is_none()); // '-stat' has a dash; /status is fine
        assert!(deny_term("/xremote").is_none());
    }
}
