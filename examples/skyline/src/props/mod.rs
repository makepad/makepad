//! Props: every turbine, lamp, pylon, house, bridge ... as instances of simple parts, built in Rust
//! from the zones whenever a parameter that places them changes, and drawn by one part shader in
//! the scene's painter's order (see [`Bucket`]).
//!
//! Owner after the foundation: stage C1 (this file and `kit.rs`); the generators are stage C2's,
//! one file each, registered in [`GENERATORS`] in the page's order so the instances come out in
//! the page's order.
pub mod kit;

mod bridge;
mod churches;
mod coast;
mod cranes;
mod fair;
mod far;
mod farms;
mod industry;
mod lamps;
mod masts;
mod pylons;
mod town;
mod turbines;

use crate::params::{Params, P};
use crate::render::SceneCtx;
use crate::zones::Zones;
use kit::{Ctx, DrawSkyPart, PropSet};
use makepad_widgets::makepad_draw::*;

/// A prop family's generator.
pub type Generator = fn(&mut Ctx);

/// The generators in the page's order (`pkBuild`).
pub const GENERATORS: [(&str, Generator); 13] = [
    ("turbines", turbines::build),
    ("lamps", lamps::build),
    ("pylons", pylons::build),
    ("industry", industry::build),
    ("cranes", cranes::build),
    ("churches", churches::build),
    ("town", town::build),
    ("bridge", bridge::build),
    ("fair", fair::build),
    ("masts", masts::build),
    ("far", far::build),
    ("farms", farms::build),
    ("coast", coast::build),
];

/// The parameters that place props (the page rebuilds them on any change but the view's).
pub const PROP_KEYS: &[P] = &[
    P::Land, P::LandC, P::MtH, P::MtW, P::MtC, P::MtRug, P::CtH, P::CtD, P::CtW, P::CtC, P::CtR, P::TrD, P::TrH, P::TrCon, P::TrW,
    P::TrC, P::Treeline, P::PrPyl, P::PrTurb, P::PrCrane, P::PrInd, P::PrChurch, P::PrTown, P::PrLamp, P::PrBridge, P::PrFair,
    P::PrRefl, P::PrRipple, P::PrMast, P::PrFar, P::PrFarm, P::PrCoast, P::Lights, P::Temp, P::WinD, P::Glow, P::Haze, P::Off, P::OffDir,
];

/// Where in the scene's painter's order a group of instances is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bucket {
    /// Just before range k: in front of range k - 1, behind range k (wind turbines on range k,
    /// masts, distant lights and pylons by their distance).
    Range(usize),
    /// After every range, before the city (industry, far cranes).
    BehindCity,
    /// After the city, before the front treeline and the ground band (the props layer).
    FrontOfCity,
    /// After the ground band: the props layer again where it stands on the water, down to each
    /// prop's own waterline (the page's pull-through over the sea band).
    OverSea,
}

/// Builds every prop of a parameter set.
pub fn build(z: &Zones, p: &Params) -> PropSet {
    let mut set = PropSet::default();
    let mut c = Ctx { z, p, set: &mut set };
    for (_, generate) in GENERATORS {
        generate(&mut c);
    }
    set
}

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.SkyPropsBase = #(Props::script_component(vm))
    mod.draw.SkyProps = mod.std.set_type_default() do mod.draw.SkyPropsBase{
        draw_part: mod.draw.DrawSkyPart{}
    }
}

#[derive(Script, ScriptHook)]
pub struct Props {
    #[live]
    pub draw_part: DrawSkyPart,
    #[rust]
    pub set: PropSet,
    #[rust]
    key: Option<u64>,
}

impl Props {
    /// Rebuilds the instances when a parameter that places them changed.
    pub fn update(&mut self, z: &Zones, p: &Params) {
        let key = p.hash(PROP_KEYS, &[]);
        if self.key != Some(key) {
            self.set = build(z, p);
            self.key = Some(key);
        }
    }

    /// Draws the instances of one bucket into the scene (`sc.mirror`: seen from the mirrored eye).
    /// Stub: nothing yet.
    pub fn draw(&mut self, _cx: &mut Cx2d, _sc: &SceneCtx, _bucket: Bucket) {}
}
