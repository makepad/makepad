//! Lighting gels (phase 1b): the sheets a gaffer clips in front of a lamp.
//!
//! A gel is a filter, so it is modelled as a linear Rec.709 transmission with
//! no channel above 1: it can only take light away. Four families:
//! - **Conversion** (CTO warms, CTB cools), stored as a mired shift. A mired
//!   is 10^6 / kelvin, and a conversion gel adds the same number of mireds to
//!   any source: that is why gaffers count in them. It is the filter that
//!   makes such a shift ([`conversion_transmission`]), on every light alike
//!   (D10, L:86): a Kelvin light lands on the shifted Kelvin's colour to the
//!   filter's accuracy, keeps its colour and level when switched to RGB, and
//!   a strong CTB keeps cooling past the Kelvin colours' 20000 K end, as a
//!   sheet in front of a lamp does.
//! - **Green** (plus green, minus green), in units of the light's own `tint`
//!   control, for matching fluorescent or LED casts.
//! - **Neutral density**: optical density d passes 10^-d (0.3 is one stop).
//! - **Colours**: ten saturated party colours as RGB transmissions.
//!
//! The names are generic ("cto_half", "nd_0.3", "amber"); no maker's
//! catalogue numbers. The mired shifts are the customary steps of the
//! full/half/quarter/eighth conversion sets.
//!
//! Limits, by design: gels act on the light's RGB, not on a spectrum (spectra
//! only derive a light's colour, see spectrum.rs), and a light carries at
//! most [`GEL_LIMIT`] of them.

use makepad_draw::*;
use super::studio::tint_rgb;

/// Most gels one light carries.
pub const GEL_LIMIT: usize = 2;

/// What a gel does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GelKind {
    /// Colour-temperature conversion as a mired shift: positive warms (CTO),
    /// negative cools (CTB).
    Conversion(f32),
    /// Green/magenta correction in units of `LightParams::tint`: negative
    /// adds green (plus green), positive takes it out (minus green).
    Green(f32),
    /// Neutral density, as optical density: it passes 10^-density.
    Density(f32),
    /// A saturated colour: linear Rec.709 transmission per channel, 0..=1.
    Colour([f32; 3]),
}

/// One gel of the table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gel {
    /// The name stored in `LightParams::gels`.
    pub name: &'static str,
    /// The name shown in the panel.
    pub label: &'static str,
    pub kind: GelKind,
}

const fn gel_row(name: &'static str, label: &'static str, kind: GelKind) -> Gel {
    Gel { name, label, kind }
}

/// Every gel, in panel order.
pub const GELS: [Gel; 27] = [
    // Conversion to orange: full takes 6500 K daylight to 3200 K tungsten.
    gel_row("cto_full", "CTO full", GelKind::Conversion(159.0)),
    gel_row("cto_half", "CTO 1/2", GelKind::Conversion(109.0)),
    gel_row("cto_quarter", "CTO 1/4", GelKind::Conversion(64.0)),
    gel_row("cto_eighth", "CTO 1/8", GelKind::Conversion(26.0)),
    // Conversion to blue: full takes 3200 K tungsten to 5700 K daylight.
    gel_row("ctb_full", "CTB full", GelKind::Conversion(-137.0)),
    gel_row("ctb_half", "CTB 1/2", GelKind::Conversion(-78.0)),
    gel_row("ctb_quarter", "CTB 1/4", GelKind::Conversion(-35.0)),
    gel_row("ctb_eighth", "CTB 1/8", GelKind::Conversion(-18.0)),
    gel_row("plus_green_full", "Plus green full", GelKind::Green(-0.75)),
    gel_row("plus_green_half", "Plus green 1/2", GelKind::Green(-0.375)),
    gel_row("plus_green_quarter", "Plus green 1/4", GelKind::Green(-0.1875)),
    gel_row("minus_green_full", "Minus green full", GelKind::Green(0.75)),
    gel_row("minus_green_half", "Minus green 1/2", GelKind::Green(0.375)),
    gel_row("minus_green_quarter", "Minus green 1/4", GelKind::Green(0.1875)),
    gel_row("nd_0.3", "ND 0.3 (1 stop)", GelKind::Density(0.3)),
    gel_row("nd_0.6", "ND 0.6 (2 stops)", GelKind::Density(0.6)),
    gel_row("nd_0.9", "ND 0.9 (3 stops)", GelKind::Density(0.9)),
    gel_row("red", "Red", GelKind::Colour([1.0, 0.05, 0.03])),
    gel_row("orange", "Orange", GelKind::Colour([1.0, 0.35, 0.03])),
    gel_row("amber", "Amber", GelKind::Colour([1.0, 0.55, 0.08])),
    gel_row("yellow", "Yellow", GelKind::Colour([1.0, 0.9, 0.08])),
    gel_row("green", "Green", GelKind::Colour([0.08, 0.85, 0.15])),
    gel_row("cyan", "Cyan", GelKind::Colour([0.05, 0.75, 0.95])),
    gel_row("blue", "Blue", GelKind::Colour([0.03, 0.12, 0.9])),
    gel_row("purple", "Purple", GelKind::Colour([0.45, 0.08, 0.9])),
    gel_row("magenta", "Magenta", GelKind::Colour([0.95, 0.08, 0.6])),
    gel_row("pink", "Pink", GelKind::Colour([1.0, 0.4, 0.65])),
];

/// The gel names, in table order: what `LightParams::gels` may hold.
pub const GEL_NAMES: [&str; 27] = {
    let mut names = [""; 27];
    let mut i = 0;
    while i < GELS.len() {
        names[i] = GELS[i].name;
        i += 1;
    }
    names
};

/// The table row for a name; spelling is forgiven (case, outer spaces).
pub fn gel(name: &str) -> Option<&'static Gel> {
    let name = name.trim();
    GELS.iter().find(|g| g.name.eq_ignore_ascii_case(name))
}

/// The known names of `names`, lowercased, the first GEL_LIMIT of them: what
/// `HdriParams::clamp` keeps of a light's gel list.
pub fn clean_names(names: &[String]) -> Vec<String> {
    names.iter().filter_map(|n| gel(n)).take(GEL_LIMIT).map(|g| g.name.to_string()).collect()
}

/// Wien's displacement per mired for the three channels: c2 * 1e-6 / lambda,
/// with c2 = 1.4388e7 nm K and effective wavelengths of 652, 550 and 440 nm.
/// The red and blue wavelengths sit outside the primaries' peaks on purpose:
/// they are fitted so that this filter moves the Planckian colours of
/// `kelvin_to_rgb` along the locus (within 0.07 per channel of the
/// luminance-1 colour while the result stays between 2000 K and 7000 K,
/// within 0.14 beyond), which the negative lobes of the Rec.709 matrix make
/// a monochromatic 610/465 nm pair miss.
const MIRED_EXPONENT: [f32; 3] = [0.022_067, 0.026_160, 0.032_700];

/// The linear RGB transmission of an ideal conversion gel of `mired`.
///
/// In Wien's approximation a blackbody at T is exp(-c2 / (lambda T)), so the
/// filter exp(-c2 * shift / lambda) turns any blackbody into the one `shift`
/// mireds further on. Scaled so its best channel passes everything: an ideal
/// gel. That costs a full CTO about 40 % of a daylight source's luminance and
/// a full CTB about 64 % of a tungsten one's, close to what real sheets lose.
pub fn conversion_transmission(mired: f32) -> Vec3f {
    let m = if mired.is_finite() { mired.clamp(-1000.0, 1000.0) } else { 0.0 };
    let e = [-MIRED_EXPONENT[0] * m, -MIRED_EXPONENT[1] * m, -MIRED_EXPONENT[2] * m];
    let top = e[0].max(e[1]).max(e[2]);
    vec3f((e[0] - top).exp(), (e[1] - top).exp(), (e[2] - top).exp())
}

impl Gel {
    /// Linear Rec.709 transmission; no channel is above 1.
    pub fn transmission(&self) -> Vec3f {
        match self.kind {
            GelKind::Conversion(mired) => conversion_transmission(mired),
            GelKind::Green(tint) => {
                // The light's own tint curve, as a filter: the channel it
                // favours passes fully and the others are cut.
                let m = tint_rgb(tint);
                m / m.max_elem().max(1.0e-6)
            }
            GelKind::Density(density) => Vec3f::all(10.0f32.powf(-density.max(0.0))),
            GelKind::Colour(rgb) => vec3f(rgb[0].clamp(0.0, 1.0), rgb[1].clamp(0.0, 1.0), rgb[2].clamp(0.0, 1.0)),
        }
    }
}

/// A light's gels, folded into what `studio::light_color` needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GelStack {
    /// Product of the conversion gels' transmissions.
    pub conversion: Vec3f,
    /// Product of every other gel's transmission (green, density, colour).
    pub filter: Vec3f,
}

impl GelStack {
    /// No gels: everything passes.
    pub const CLEAR: GelStack = GelStack { conversion: Vec3f::all(1.0), filter: Vec3f::all(1.0) };

    /// The whole stack as one RGB transmission.
    pub fn transmission(&self) -> Vec3f {
        self.conversion * self.filter
    }
}

/// Folds gel names into a stack. Unknown names are skipped and only the
/// first GEL_LIMIT known ones count, as `clean_names` keeps them. The order
/// does not matter: filters multiply.
pub fn gel_stack(names: &[String]) -> GelStack {
    let mut stack = GelStack::CLEAR;
    for gel in names.iter().filter_map(|n| gel(n)).take(GEL_LIMIT) {
        match gel.kind {
            GelKind::Conversion(_) => stack.conversion = stack.conversion * gel.transmission(),
            _ => stack.filter = stack.filter * gel.transmission(),
        }
    }
    stack
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdri::studio::{kelvin_to_rgb, light_color};
    use crate::hdri::LightParams;
    use crate::sky::luminance;

    fn with_gels(light: &LightParams, gels: &[&str]) -> Vec3f {
        let mut l = light.clone();
        l.gels = Some(gels.iter().map(|g| g.to_string()).collect());
        light_color(&l)
    }

    fn chroma(c: Vec3f) -> Vec3f {
        c / luminance(c)
    }

    fn kelvin_light(kelvin: f32) -> LightParams {
        LightParams { kelvin, intensity_ev: 0.0, ..LightParams::default() }
    }

    /// The exact shift a conversion gel's filter stands for: `kelvin` moved
    /// by `mired` (10^6 / K).
    fn shifted(kelvin: f32, mired: f32) -> f32 {
        1.0e6 / (1.0e6 / kelvin + mired)
    }

    #[test]
    fn the_table_is_complete_and_the_names_are_plain() {
        assert_eq!(GEL_NAMES.len(), GELS.len());
        let mut seen = std::collections::HashSet::new();
        for (name, row) in GEL_NAMES.iter().zip(GELS.iter()) {
            assert_eq!(*name, row.name);
            assert!(seen.insert(*name), "{name} is listed twice");
            assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.'), "{name}");
            assert!(!row.label.is_empty());
            assert_eq!(gel(name), Some(row));
            let t = row.transmission();
            assert!(t.is_finite() && t.min_elem() >= 0.0 && t.max_elem() <= 1.0, "{name}: {t:?}");
        }
        let count = |f: fn(&GelKind) -> bool| GELS.iter().filter(|g| f(&g.kind)).count();
        assert_eq!(count(|k| matches!(k, GelKind::Conversion(_))), 8, "CTO and CTB: full, half, quarter, eighth");
        assert_eq!(count(|k| matches!(k, GelKind::Green(_))), 6, "plus and minus green: full, half, quarter");
        assert_eq!(count(|k| matches!(k, GelKind::Density(_))), 3);
        assert_eq!(count(|k| matches!(k, GelKind::Colour(_))), 10);
        // Spelling is forgiven; unknown names are not gels.
        assert_eq!(gel(" CTO_Full ").map(|g| g.name), Some("cto_full"));
        assert_eq!(gel("lee 204"), None);
        assert_eq!(clean_names(&["ND_0.3".to_string(), "fog".to_string(), "red".to_string(), "blue".to_string()]), ["nd_0.3", "red"]);
    }

    #[test]
    fn nd_03_halves_the_light() {
        let light = kelvin_light(4300.0);
        let bare = light_color(&light);
        for (name, density) in [("nd_0.3", 0.3f32), ("nd_0.6", 0.6), ("nd_0.9", 0.9)] {
            let c = with_gels(&light, &[name]);
            let ratio = luminance(c) / luminance(bare);
            assert!((ratio - 10.0f32.powf(-density)).abs() < 1.0e-5, "{name}: {ratio}");
            assert!((chroma(c) - chroma(bare)).length() < 1.0e-5, "{name} is neutral");
        }
        let half = luminance(with_gels(&light, &["nd_0.3"])) / luminance(bare);
        assert!((half - 0.5).abs() < 0.002, "ND 0.3 is one stop: {half}");
    }

    #[test]
    fn full_cto_takes_daylight_to_tungsten_and_full_ctb_back() {
        let daylight = kelvin_light(6500.0);
        let warm = with_gels(&daylight, &["cto_full"]);
        let k = shifted(6500.0, 159.0);
        assert!((3190.0..3205.0).contains(&k), "{k}");
        // The filter lands on the shifted Kelvin's hue to its own accuracy
        // (MIRED_EXPONENT: 0.07 per channel from 2000 to 7000 K).
        assert!((chroma(warm) - kelvin_to_rgb(k)).abs().max_elem() < 0.07, "the hue is the shifted Kelvin's: {warm:?}");
        let kept = luminance(warm) / luminance(light_color(&daylight));
        assert!((0.5..0.7).contains(&kept), "a full CTO passes about 60 %: {kept}");

        let tungsten = kelvin_light(3200.0);
        let cool = with_gels(&tungsten, &["ctb_full"]);
        let k = shifted(3200.0, -137.0);
        assert!((5680.0..5710.0).contains(&k), "{k}");
        assert!((chroma(cool) - kelvin_to_rgb(k)).abs().max_elem() < 0.07, "{cool:?}");
        let kept = luminance(cool) / luminance(light_color(&tungsten));
        assert!((0.3..0.45).contains(&kept), "a full CTB passes about 36 %: {kept}");
        // No end to stop at (D10): a_conversion_gel_is_one_filter_on_every_light.
    }

    #[test]
    fn the_fractions_step_from_eighth_to_full() {
        let light = kelvin_light(5600.0);
        let bare = light_color(&light);
        let mut last_warmth = bare.x / bare.z;
        let mut last_level = luminance(bare);
        for name in ["cto_eighth", "cto_quarter", "cto_half", "cto_full"] {
            let c = with_gels(&light, &[name]);
            assert!(c.x / c.z > last_warmth, "{name} is warmer than the step before");
            assert!(luminance(c) < last_level, "{name} costs more light than the step before");
            last_warmth = c.x / c.z;
            last_level = luminance(c);
        }
        let mut last_warmth = bare.x / bare.z;
        for name in ["ctb_eighth", "ctb_quarter", "ctb_half", "ctb_full"] {
            let c = with_gels(&light, &[name]);
            assert!(c.x / c.z < last_warmth, "{name} is cooler than the step before");
            last_warmth = c.x / c.z;
        }
    }

    #[test]
    fn green_gels_move_green_against_red_and_blue() {
        let light = kelvin_light(5600.0);
        let bare = light_color(&light);
        let plus = with_gels(&light, &["plus_green_full"]);
        let minus = with_gels(&light, &["minus_green_full"]);
        assert!(plus.y / plus.x > 1.5 * bare.y / bare.x, "plus green: {plus:?}");
        assert!(minus.y / minus.x < 0.7 * bare.y / bare.x, "minus green: {minus:?}");
        // Red and blue are cut alike, so the warmth stays.
        assert!((plus.x / plus.z - bare.x / bare.z).abs() < 1.0e-4);
        assert!((minus.x / minus.z - bare.x / bare.z).abs() < 1.0e-4);
        let half = with_gels(&light, &["plus_green_half"]);
        assert!(half.y / half.x > bare.y / bare.x && half.y / half.x < plus.y / plus.x);
    }

    #[test]
    fn no_gel_brightens_a_light() {
        for name in GEL_NAMES {
            for kelvin in [2000.0f32, 3200.0, 5600.0, 6500.0, 10000.0] {
                let light = kelvin_light(kelvin);
                let (bare, c) = (light_color(&light), with_gels(&light, &[name]));
                assert!(c.is_finite() && c.min_elem() >= 0.0, "{name} at {kelvin} K: {c:?}");
                assert!(luminance(c) <= luminance(bare) * 1.0001, "{name} at {kelvin} K: {} > {}", luminance(c), luminance(bare));
            }
            // On every light the gel is a plain filter: no channel grows.
            for light in [
                LightParams { rgb: Some([0.9, 0.6, 0.3]), intensity_ev: 2.0, ..LightParams::default() },
                kelvin_light(1200.0),
                kelvin_light(1900.0),
                kelvin_light(15000.0),
            ] {
                let (bare, c) = (light_color(&light), with_gels(&light, &[name]));
                assert!(c.x <= bare.x * 1.0001 && c.y <= bare.y * 1.0001 && c.z <= bare.z * 1.0001, "{name}: {c:?} vs {bare:?}");
            }
        }
    }

    #[test]
    fn two_gels_stack_in_any_order_and_a_third_is_ignored() {
        let light = kelvin_light(5600.0);
        let a = with_gels(&light, &["cto_half", "nd_0.6"]);
        let b = with_gels(&light, &["nd_0.6", "cto_half"]);
        assert!((a - b).length() < 1.0e-6);
        // Two densities add up.
        let two = with_gels(&light, &["nd_0.3", "nd_0.6"]);
        let one = with_gels(&light, &["nd_0.9"]);
        assert!((two - one).length() < 1.0e-5, "{two:?} vs {one:?}");
        // Two conversions add their mireds (their filters multiply to the
        // filter of the sum), and each takes its own light.
        let both = with_gels(&light, &["cto_half", "cto_quarter"]);
        assert!((chroma(both) - chroma(kelvin_to_rgb(5600.0) * conversion_transmission(109.0 + 64.0))).length() < 1.0e-5);
        assert!(luminance(both) < luminance(with_gels(&light, &["cto_half"])));
        // Only GEL_LIMIT gels count, and unknown names do not use up a slot.
        assert_eq!(with_gels(&light, &["red", "nd_0.3", "blue"]), with_gels(&light, &["red", "nd_0.3"]));
        assert_eq!(with_gels(&light, &["gobo", "red"]), with_gels(&light, &["red"]));
        assert_eq!(gel_stack(&[]), GelStack::CLEAR);
        let stack = gel_stack(&["ctb_quarter".to_string(), "green".to_string()]);
        assert_eq!(stack.conversion, conversion_transmission(-35.0));
        assert!((stack.transmission() - stack.conversion * stack.filter).length() < 1.0e-7);
    }

    #[test]
    fn a_light_without_gels_is_untouched() {
        for light in [
            LightParams::default(),
            LightParams { kelvin: 3200.0, tint: 0.3, ..LightParams::default() },
            LightParams { rgb: Some([0.2, 0.4, 0.6]), intensity_ev: 1.0, ..LightParams::default() },
        ] {
            let none = light_color(&light);
            let empty = light_color(&LightParams { gels: Some(Vec::new()), ..light.clone() });
            assert_eq!(none, empty);
        }
        // The colour a bare default light had before gels existed: 6500 K at +3 EV.
        let c = light_color(&LightParams::default());
        assert_eq!(c, kelvin_to_rgb(6500.0) * tint_rgb(0.0) / luminance(kelvin_to_rgb(6500.0) * tint_rgb(0.0)).max(1.0e-6) * 8.0);
    }

    /// D10 (L:86): a conversion gel is one filter on every light. A Kelvin
    /// light behind it keeps its colour and level when switched to RGB (its
    /// luminance-1 colour, tint included, as an explicit colour: the panel's
    /// switch), and a strong CTB is not stopped at 20000 K.
    #[test]
    fn a_conversion_gel_is_one_filter_on_every_light() {
        for (kelvin, tint, gels) in [
            (6500.0f32, 0.0f32, &["ctb_full"][..]),
            (5600.0, 0.0, &["ctb_full"][..]),
            (10000.0, 0.0, &["ctb_half"][..]),
            (4500.0, 0.0, &["ctb_full"][..]),
            (2700.0, 1.0, &["cto_full"][..]),
            (3200.0, -0.5, &["ctb_half", "nd_0.3"][..]),
            (15000.0, 0.3, &["ctb_eighth", "cto_quarter"][..]),
        ] {
            let light = LightParams { kelvin, tint, intensity_ev: 1.0, ..LightParams::default() };
            let c = kelvin_to_rgb(kelvin) * tint_rgb(tint);
            let c = c / luminance(c);
            // As the panel's switch stores it (`switch_light_color_mode`): an
            // explicit colour is 0..=1 per channel (clamp(), which
            // `light_color` reads it through, D17), so the peak of a
            // luminance-1 colour above 1 (6500 K's blue) moves into the level.
            let peak = c.max_elem().max(1.0);
            let switched = LightParams { rgb: Some([c.x / peak, c.y / peak, c.z / peak]), intensity_ev: light.intensity_ev + peak.log2(), ..light.clone() };
            let (as_kelvin, as_rgb) = (with_gels(&light, gels), with_gels(&switched, gels));
            assert!((as_kelvin - as_rgb).abs().max_elem() < 1.0e-5 * as_rgb.max_elem(), "{kelvin} K, tint {tint}, {gels:?}: {as_kelvin:?} as Kelvin, {as_rgb:?} as RGB");
        }
        // A full CTB keeps 5600, 6500 and 10000 K apart, each bluer than the
        // last, and takes 6500 K past the bluest Kelvin colour, 20000 K.
        let blue = |kelvin: f32| {
            let c = with_gels(&kelvin_light(kelvin), &["ctb_full"]);
            c.z / c.x
        };
        assert!(blue(5600.0) < blue(6500.0) && blue(6500.0) < blue(10000.0), "{} {} {}", blue(5600.0), blue(6500.0), blue(10000.0));
        let end = kelvin_to_rgb(20000.0);
        assert!(blue(6500.0) > 1.2 * end.z / end.x, "{} vs 20000 K's {}", blue(6500.0), end.z / end.x);
    }

    #[test]
    fn the_conversion_filter_follows_the_planckian_locus() {
        // The filter stands in for the exact Kelvin shift on every light: it
        // must land near the shifted Kelvin's colour.
        for (kelvin, mired) in [(6500.0f32, 159.0f32), (5600.0, 109.0), (3200.0, -137.0), (2700.0, -78.0), (4500.0, 64.0)] {
            let filtered = chroma(kelvin_to_rgb(kelvin) * conversion_transmission(mired));
            let exact = kelvin_to_rgb(shifted(kelvin, mired));
            assert!((filtered - exact).abs().max_elem() < 0.07, "{kelvin} K {mired:+}: {filtered:?} vs {exact:?}");
        }
        assert_eq!(conversion_transmission(0.0), Vec3f::all(1.0));
        assert_eq!(conversion_transmission(f32::NAN), Vec3f::all(1.0));
    }
}
