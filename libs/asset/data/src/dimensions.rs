//! How BIG a thing is, in metres, and the named sizes a level may draw it at.
//!
//! Every importer — the classic converters, the Kenney/pack compiler, the AI
//! mesh publish — funnels through this one type so that a Doom imp, a Kenney
//! knight and a Trellis-generated golem all state their size in the same
//! words. The engine's world unit is the metre and the MR stage renders at
//! scale 1.0, so the yardstick is a standing human of [`PERSON_HEIGHT`].
//!
//! Two facts are kept apart on purpose:
//!
//! - **Metric truth.** `metres_per_unit` is the calibration of the asset's
//!   native model units (`1 / CoordinateSystem::units_per_meter`), and
//!   `width/height/length/radius/eye` are the asset's REAL-WORLD extents at
//!   that calibration. A Kenney mini-character is 0.7 native units tall and
//!   represents a 1.75 m person, so its `metres_per_unit` is 2.5 and its
//!   `height` is 1.75. Nothing about how a level chooses to draw it lives
//!   here.
//! - **Presets.** [`ScalePreset`] names the sizes a level, an AI verb or a
//!   held-item path may pick: `real` (measured), `comic` (the pack's own
//!   authored play size — Kenney cars are deliberately short so they are fun
//!   to drive), `small` (a toy on the ground you can kick or push) and
//!   `handheld` (fits a 0.2–0.4 m grip box: a toy car in the hand, a prop
//!   wielded as a weapon or pickup). Each preset is a uniform factor in
//!   **metres per native unit** — the one number an engine multiplies the
//!   GLB by — and every one of them is derived by the rules in
//!   [`Dimensions::measure`] from the metric truth plus the asset's class.
//!
//! The text form (`asset-dimensions 1`) is the sidecar the converters and
//! the AI publish write beside a mesh; [`Dimensions::anchors`] is the same
//! data folded into a manifest's named anchors for packs whose upload plan
//! cannot carry a generated file. Readers accept either.

use crate::asset::{Anchor, AssetKind};
use crate::geom::{Quat, Transform, Vec3};

pub const MAGIC: &str = "asset-dimensions";
pub const VERSION: u32 = 1;
pub const CONTENT_TYPE: &str = "text/x-asset-dimensions";

/// A standing human, metres: the yardstick every importer pins its
/// character class to (Doom's 56-unit marine, Quake's 56-unit player, a
/// Kenney mini-character, a generated hero).
pub const PERSON_HEIGHT: f32 = 1.75;
/// Where a standing human's eyes sit, as a fraction of height (1.65 / 1.75).
pub const PERSON_EYE_RATIO: f32 = 1.65 / PERSON_HEIGHT;
/// A standing human's body radius, as a fraction of height (0.35 / 1.75).
pub const PERSON_RADIUS_RATIO: f32 = 0.35 / PERSON_HEIGHT;
/// `small`: the largest extent of a toy you can kick along the floor.
pub const SMALL_MAX_EXTENT: f32 = 0.4;
/// `handheld`: the largest extent of something that fits a hand's grip box.
pub const HANDHELD_MAX_EXTENT: f32 = 0.25;
pub const CHARACTER_MASS_KG: f32 = 80.0;
pub const VEHICLE_MASS_KG: f32 = 1400.0;
pub const PROP_DENSITY_KG_M3: f32 = 200.0;
pub const PROP_MASS_CAP_KG: f32 = 500.0;

/// Anchor names the manifest carrier uses. Heights ride in `pos.y` (the
/// convention `world_nav` established for `floor_height` / `eye_height`);
/// scale factors ride in `scale`, uniformly.
pub const ANCHOR_HEIGHT: &str = "dim_height";
pub const ANCHOR_RADIUS: &str = "dim_radius";
pub const ANCHOR_EYE: &str = "dim_eye";
pub const ANCHOR_SCALE_REAL: &str = "scale_real";
pub const ANCHOR_SCALE_COMIC: &str = "scale_comic";
pub const ANCHOR_SCALE_SMALL: &str = "scale_small";
pub const ANCHOR_SCALE_HANDHELD: &str = "scale_handheld";

/// The named sizes a level may draw an asset at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScalePreset {
    /// Measured real-world size.
    #[default]
    Real,
    /// The pack's authored play size: the GLB's own units taken as metres.
    /// For an asset authored in metres this equals `real`.
    Comic,
    /// A toy on the ground: largest extent [`SMALL_MAX_EXTENT`].
    Small,
    /// Fits the hand: largest extent [`HANDHELD_MAX_EXTENT`]. A weapon's
    /// handheld size IS its real size.
    Handheld,
}

impl ScalePreset {
    pub const ALL: [ScalePreset; 4] = [Self::Real, Self::Comic, Self::Small, Self::Handheld];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Real => "real",
            Self::Comic => "comic",
            Self::Small => "small",
            Self::Handheld => "handheld",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "real" => Self::Real,
            "comic" => Self::Comic,
            "small" => Self::Small,
            "handheld" => Self::Handheld,
            _ => return None,
        })
    }

    fn anchor_name(self) -> &'static str {
        match self {
            Self::Real => ANCHOR_SCALE_REAL,
            Self::Comic => ANCHOR_SCALE_COMIC,
            Self::Small => ANCHOR_SCALE_SMALL,
            Self::Handheld => ANCHOR_SCALE_HANDHELD,
        }
    }
}

/// What the size rules treat an asset as. Coarser than [`AssetKind`]: the
/// rules only care whether the thing is a body, a ride, something wielded,
/// or scenery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeClass {
    Character,
    Vehicle,
    Weapon,
    Prop,
    World,
}

impl SizeClass {
    pub fn of_kind(kind: AssetKind) -> SizeClass {
        match kind {
            AssetKind::Character => Self::Character,
            AssetKind::Vehicle => Self::Vehicle,
            AssetKind::Weapon => Self::Weapon,
            AssetKind::World => Self::World,
            _ => Self::Prop,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::Vehicle => "vehicle",
            Self::Weapon => "weapon",
            Self::Prop => "prop",
            Self::World => "world",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "character" => Self::Character,
            "vehicle" => Self::Vehicle,
            "weapon" => Self::Weapon,
            "prop" | "mesh" => Self::Prop,
            "world" => Self::World,
            _ => return None,
        })
    }

    /// The preset a level draws this class at when it names none.
    /// Characters, weapons, scenery and worlds are real by default so a
    /// mashup composes at one metre; vehicles default to their authored
    /// play size, because a real-sized Kenney car is a stretched sedan
    /// and a comic one is fun to drive.
    pub fn default_preset(self) -> ScalePreset {
        match self {
            Self::Vehicle => ScalePreset::Comic,
            _ => ScalePreset::Real,
        }
    }
}

/// The size of one asset: metric truth plus the preset factors derived
/// from it. See the module doc for what each field means.
#[derive(Clone, Debug, PartialEq)]
pub struct Dimensions {
    pub class: SizeClass,
    /// Calibration: metres per native model unit (`1 / units_per_meter`).
    pub metres_per_unit: f32,
    /// Real-world extent along the model's X, in metres.
    pub width: f32,
    /// Real-world extent along the model's up axis, in metres.
    pub height: f32,
    /// Real-world extent along the model's forward axis, in metres.
    pub length: f32,
    /// Body radius in metres (characters), else 0.
    pub radius: f32,
    /// Eye height above the feet in metres (characters), else 0.
    pub eye: f32,
    /// Optional authored physical weight. Older sidecars omit it; readers
    /// fill the class default when the dimensions contain enough facts.
    pub mass_kg: Option<f32>,
    /// Preset factors, metres per native unit, in [`ScalePreset::ALL`] order.
    pub factors: [f32; 4],
    /// The preset to draw at when the level names none.
    pub default: ScalePreset,
    /// What pinned the calibration — informative, free text without newlines
    /// (`"doom 56-unit marine = 1.75 m"`, `"kenney mini-characters 0.70 = 1.75 m"`).
    pub pin: String,
}

impl Dimensions {
    fn default_mass(class: SizeClass, width: f32, height: f32, length: f32) -> Option<f32> {
        match class {
            SizeClass::Character => Some(CHARACTER_MASS_KG),
            SizeClass::Vehicle => Some(VEHICLE_MASS_KG),
            SizeClass::Prop | SizeClass::Weapon => {
                let volume = width.max(0.0) * height.max(0.0) * length.max(0.0);
                (volume > 0.0).then_some((volume * PROP_DENSITY_KG_M3).min(PROP_MASS_CAP_KG))
            }
            SizeClass::World => None,
        }
    }

    pub fn effective_mass_kg(&self) -> Option<f32> {
        self.mass_kg.or_else(|| Self::default_mass(self.class, self.width, self.height, self.length))
    }

    /// Derive everything from the calibration and the native extents.
    ///
    /// `native_extent` is `(max - min)` of the model bounds in its own units,
    /// `[x, y, z]` with Y up. The rules:
    ///
    /// - `real`     = `metres_per_unit`
    /// - `comic`    = 1.0 (the authored units drawn as metres)
    /// - `small`    = the smaller of `real` and what puts the largest extent at
    ///   [`SMALL_MAX_EXTENT`] — a thing already smaller than a toy is not grown
    /// - `handheld` = `real` for a weapon (it is wielded at its own size),
    ///   else the smaller of `real` and the [`HANDHELD_MAX_EXTENT`] fit
    /// - characters get `radius` and `eye` from the standing-human ratios
    pub fn measure(class: SizeClass, metres_per_unit: f32, native_extent: [f32; 3], pin: &str) -> Dimensions {
        let mpu = if metres_per_unit.is_finite() && metres_per_unit > 0.0 { metres_per_unit } else { 1.0 };
        let ext = native_extent.map(|e| if e.is_finite() && e > 0.0 { e } else { 0.0 });
        let largest = ext[0].max(ext[1]).max(ext[2]);
        let fit = |max_extent: f32| -> f32 {
            if largest > 0.0 { mpu.min(max_extent / largest) } else { mpu }
        };
        let real = mpu;
        let comic = 1.0;
        let small = fit(SMALL_MAX_EXTENT);
        let handheld = match class {
            SizeClass::Weapon => real,
            _ => fit(HANDHELD_MAX_EXTENT),
        };
        let height = ext[1] * mpu;
        let (radius, eye) = match class {
            SizeClass::Character if height > 0.0 => (height * PERSON_RADIUS_RATIO, height * PERSON_EYE_RATIO),
            _ => (0.0, 0.0),
        };
        Dimensions {
            class,
            metres_per_unit: mpu,
            width: ext[0] * mpu,
            height,
            length: ext[2] * mpu,
            radius,
            eye,
            mass_kg: Self::default_mass(class, ext[0] * mpu, height, ext[2] * mpu),
            factors: [real, comic, small, handheld],
            default: class.default_preset(),
            pin: pin.lines().next().unwrap_or("").trim().to_string(),
        }
    }

    /// Metres per native unit for `preset`: the uniform scale to draw at.
    pub fn factor(&self, preset: ScalePreset) -> f32 {
        self.factors[ScalePreset::ALL.iter().position(|p| *p == preset).unwrap_or(0)]
    }

    /// The factor for the preset a level did not name.
    pub fn default_factor(&self) -> f32 {
        self.factor(self.default)
    }

    /// The asset's own `units_per_meter` for its manifest coordinate system.
    pub fn units_per_meter(&self) -> f32 {
        1.0 / self.metres_per_unit
    }

    /// The sidecar text: one fact per line, presets as `scale <name> <f>`.
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "{MAGIC} {VERSION}\nclass {}\nmetres_per_unit {:.6}\nwidth {:.4}\nheight {:.4}\nlength {:.4}\n",
            self.class.as_str(),
            self.metres_per_unit,
            self.width,
            self.height,
            self.length
        );
        if self.radius > 0.0 {
            out.push_str(&format!("radius {:.4}\n", self.radius));
        }
        if self.eye > 0.0 {
            out.push_str(&format!("eye {:.4}\n", self.eye));
        }
        if let Some(mass) = self.mass_kg {
            out.push_str(&format!("mass_kg {:.3}\n", mass));
        }
        for (i, p) in ScalePreset::ALL.iter().enumerate() {
            out.push_str(&format!("scale {} {:.6}\n", p.as_str(), self.factors[i]));
        }
        out.push_str(&format!("default {}\n", self.default.as_str()));
        if !self.pin.is_empty() {
            out.push_str(&format!("pin {}\n", self.pin));
        }
        out
    }

    pub fn parse(text: &str) -> Result<Dimensions, String> {
        let mut lines = text.lines();
        let header = lines.next().unwrap_or("").trim();
        if !header.starts_with(MAGIC) {
            return Err("not an asset-dimensions document".into());
        }
        let mut d = Dimensions {
            class: SizeClass::Prop,
            metres_per_unit: 1.0,
            width: 0.0,
            height: 0.0,
            length: 0.0,
            radius: 0.0,
            eye: 0.0,
            mass_kg: None,
            factors: [1.0; 4],
            default: ScalePreset::Real,
            pin: String::new(),
        };
        let mut saw_class = false;
        let num = |s: Option<&str>, what: &str| -> Result<f32, String> {
            let v: f32 = s.ok_or_else(|| format!("{what}: missing value"))?
                .parse()
                .map_err(|_| format!("{what}: not a number"))?;
            if !v.is_finite() || v < 0.0 {
                return Err(format!("{what}: out of range"));
            }
            Ok(v)
        };
        for line in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split_whitespace();
            match parts.next() {
                Some("class") => {
                    d.class = SizeClass::parse(parts.next().unwrap_or("")).ok_or("class: unknown")?;
                    saw_class = true;
                }
                Some("metres_per_unit") => {
                    d.metres_per_unit = num(parts.next(), "metres_per_unit")?;
                    if d.metres_per_unit <= 0.0 {
                        return Err("metres_per_unit: must be positive".into());
                    }
                }
                Some("width") => d.width = num(parts.next(), "width")?,
                Some("height") => d.height = num(parts.next(), "height")?,
                Some("length") => d.length = num(parts.next(), "length")?,
                Some("radius") => d.radius = num(parts.next(), "radius")?,
                Some("eye") => d.eye = num(parts.next(), "eye")?,
                Some("mass_kg") => {
                    let mass = num(parts.next(), "mass_kg")?;
                    if mass <= 0.0 {
                        return Err("mass_kg: must be positive".into());
                    }
                    d.mass_kg = Some(mass);
                }
                Some("scale") => {
                    let name = parts.next().unwrap_or("");
                    let Some(p) = ScalePreset::parse(name) else {
                        // A preset this reader does not know is not an error:
                        // the vocabulary may grow.
                        continue;
                    };
                    let f = num(parts.next(), "scale")?;
                    if f <= 0.0 {
                        return Err("scale: must be positive".into());
                    }
                    d.factors[ScalePreset::ALL.iter().position(|q| *q == p).unwrap_or(0)] = f;
                }
                Some("default") => {
                    d.default = ScalePreset::parse(parts.next().unwrap_or("")).ok_or("default: unknown preset")?;
                }
                Some("pin") => d.pin = line["pin".len()..].trim().to_string(),
                // Unknown lines are future facts; old readers skip them.
                _ => {}
            }
        }
        if !saw_class {
            return Err("class: missing".into());
        }
        if d.mass_kg.is_none() {
            d.mass_kg = Self::default_mass(d.class, d.width, d.height, d.length);
        }
        Ok(d)
    }

    /// The same facts as manifest anchors, for the pack pipeline whose upload
    /// plan is "files on disk" and cannot grow a generated sidecar.
    pub fn anchors(&self) -> Vec<Anchor> {
        let height = |name: &str, v: f32| Anchor {
            name: name.into(),
            transform: Transform {
                pos: Vec3::new(0.0, v, 0.0),
                rot: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        };
        let mut out = vec![height(ANCHOR_HEIGHT, self.height)];
        if self.radius > 0.0 {
            out.push(height(ANCHOR_RADIUS, self.radius));
        }
        if self.eye > 0.0 {
            out.push(height(ANCHOR_EYE, self.eye));
        }
        for (i, p) in ScalePreset::ALL.iter().enumerate() {
            let f = self.factors[i];
            out.push(Anchor {
                name: p.anchor_name().into(),
                transform: Transform {
                    pos: Vec3::ZERO,
                    rot: Quat::IDENTITY,
                    scale: Vec3::new(f, f, f),
                },
            });
        }
        out
    }

    /// Read the anchors [`Dimensions::anchors`] wrote. `None` when the
    /// manifest predates them (no `dim_height` anchor).
    pub fn from_anchors(kind: AssetKind, units_per_meter: f32, anchors: &[Anchor]) -> Option<Dimensions> {
        let find = |name: &str| anchors.iter().find(|a| a.name == name);
        let height = find(ANCHOR_HEIGHT)?.transform.pos.y;
        let class = SizeClass::of_kind(kind);
        let mpu = if units_per_meter.is_finite() && units_per_meter > 0.0 { 1.0 / units_per_meter } else { 1.0 };
        let mut factors = [mpu, 1.0, mpu, mpu];
        for (i, p) in ScalePreset::ALL.iter().enumerate() {
            if let Some(a) = find(p.anchor_name()) {
                factors[i] = a.transform.scale.y;
            }
        }
        Some(Dimensions {
            class,
            metres_per_unit: mpu,
            width: 0.0,
            height,
            length: 0.0,
            radius: find(ANCHOR_RADIUS).map(|a| a.transform.pos.y).unwrap_or(0.0),
            eye: find(ANCHOR_EYE).map(|a| a.transform.pos.y).unwrap_or(0.0),
            mass_kg: Self::default_mass(class, 0.0, height, 0.0),
            factors,
            default: class.default_preset(),
            pin: String::new(),
        })
    }

    /// `true` when this is one of the anchors [`Dimensions::anchors`] writes.
    pub fn is_dimension_anchor(name: &str) -> bool {
        matches!(
            name,
            ANCHOR_HEIGHT
                | ANCHOR_RADIUS
                | ANCHOR_EYE
                | ANCHOR_SCALE_REAL
                | ANCHOR_SCALE_COMIC
                | ANCHOR_SCALE_SMALL
                | ANCHOR_SCALE_HANDHELD
        )
    }
}

/// The metric hint an AI "expand" step attaches to a generation request,
/// in the same words the sidecar uses. A generated mesh is unitless, so its
/// size can only come from here; the publish then calibrates the mesh to
/// it exactly as an importer calibrates a pack (`metres_per_unit =
/// height / native_height`).
///
/// JSON spelling (the field names the expand output must carry):
/// `{"height": 1.75, "length": 4.5, "width": 1.8, "class": "character",
/// "preset": "real"}` under a `dimensions` object — `height` in metres is
/// the one required key; `length` is used instead when the class is a
/// vehicle (a car is calibrated by its length, not its roof).
#[derive(Clone, Debug, PartialEq)]
pub struct SizeHint {
    pub class: SizeClass,
    /// Metres. Zero = not given.
    pub height: f32,
    pub length: f32,
    pub width: f32,
    /// The preset to draw at by default; `None` = the class rule.
    pub preset: Option<ScalePreset>,
}

impl SizeHint {
    /// Calibrate a native extent to this hint: the metres-per-unit that puts
    /// the hint's dominant dimension at its stated metres. Vehicles pin by
    /// length, everything else by height; a missing dominant dimension falls
    /// back to whichever of the three was given. `None` when nothing was.
    pub fn metres_per_unit(&self, native_extent: [f32; 3]) -> Option<f32> {
        let order: [(f32, f32); 3] = match self.class {
            SizeClass::Vehicle => [(self.length, native_extent[2]), (self.height, native_extent[1]), (self.width, native_extent[0])],
            _ => [(self.height, native_extent[1]), (self.length, native_extent[2]), (self.width, native_extent[0])],
        };
        order
            .iter()
            .find(|(metres, native)| *metres > 0.0 && *native > 0.0 && metres.is_finite() && native.is_finite())
            .map(|(metres, native)| metres / native)
    }

    /// The full dimensions of a mesh with `native_extent` calibrated to this
    /// hint. `None` when the hint carries no usable size.
    pub fn measure(&self, native_extent: [f32; 3], pin: &str) -> Option<Dimensions> {
        let mpu = self.metres_per_unit(native_extent)?;
        let mut d = Dimensions::measure(self.class, mpu, native_extent, pin);
        if let Some(p) = self.preset {
            d.default = p;
        }
        Some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn a_kenney_mini_character_measures_as_a_person() {
        // 0.70 native units tall, calibrated at 2.5 m per unit.
        let d = Dimensions::measure(SizeClass::Character, 2.5, [0.31, 0.70, 0.17], "kenney mini 0.70 = 1.75 m");
        assert!(near(d.height, 1.75), "{d:?}");
        assert!(near(d.radius, 0.35), "{d:?}");
        assert!(near(d.eye, 1.65), "{d:?}");
        assert_eq!(d.mass_kg, Some(CHARACTER_MASS_KG));
        assert!(near(d.factor(ScalePreset::Real), 2.5));
        assert!(near(d.factor(ScalePreset::Comic), 1.0));
        // A toy: 0.4 m tall → 0.4 / 0.70.
        assert!(near(d.factor(ScalePreset::Small), 0.4 / 0.70));
        assert!(near(d.factor(ScalePreset::Handheld), 0.25 / 0.70));
        assert_eq!(d.default, ScalePreset::Real);
        assert!(near(d.units_per_meter(), 0.4));
    }

    #[test]
    fn a_kenney_car_defaults_to_its_comic_play_size() {
        // The car-kit sedan: 1.5 × 1.3 × 2.55 native, calibrated by length
        // to a 4.5 m car.
        let d = Dimensions::measure(SizeClass::Vehicle, 4.5 / 2.55, [1.5, 1.3, 2.55], "car-kit sedan 2.55 = 4.5 m");
        assert!(near(d.length, 4.5), "{d:?}");
        assert_eq!(d.default, ScalePreset::Comic);
        assert!(near(d.default_factor(), 1.0));
        assert!(near(d.factor(ScalePreset::Real), 4.5 / 2.55));
        assert!(near(d.factor(ScalePreset::Handheld), 0.25 / 2.55));
        assert_eq!(d.radius, 0.0);
        assert_eq!(d.mass_kg, Some(VEHICLE_MASS_KG));
    }

    #[test]
    fn a_weapon_is_wielded_at_its_real_size_and_a_tiny_thing_is_not_grown() {
        let gun = Dimensions::measure(SizeClass::Weapon, 1.0, [0.18, 0.46, 0.80], "");
        assert!(near(gun.factor(ScalePreset::Handheld), 1.0));
        assert!(near(gun.factor(ScalePreset::Small), 0.4 / 0.80));
        // A 7 cm maki at 0.35 m/unit (a giant-food pack): already handheld.
        let maki = Dimensions::measure(SizeClass::Prop, 0.35, [0.12, 0.07, 0.10], "");
        assert!(near(maki.factor(ScalePreset::Handheld), 0.35));
        assert!(near(maki.factor(ScalePreset::Small), 0.35));
    }

    #[test]
    fn text_round_trips() {
        let d = Dimensions::measure(SizeClass::Character, 2.5, [0.31, 0.70, 0.17], "kenney mini-characters 0.70 = 1.75 m");
        let text = d.to_text();
        assert!(text.starts_with("asset-dimensions 1\nclass character\n"), "{text}");
        assert!(text.contains("height 1.7500\n"), "{text}");
        assert!(text.contains("scale real 2.500000\n"), "{text}");
        assert!(text.contains("default real\n"), "{text}");
        assert!(text.contains("mass_kg 80.000\n"), "{text}");
        let back = Dimensions::parse(&text).unwrap();
        assert_eq!(back.class, d.class);
        assert!(near(back.metres_per_unit, d.metres_per_unit));
        assert!(near(back.height, d.height));
        assert!(near(back.radius, d.radius));
        assert!(near(back.eye, d.eye));
        assert_eq!(back.mass_kg, d.mass_kg);
        for p in ScalePreset::ALL {
            assert!(near(back.factor(p), d.factor(p)), "{p:?}");
        }
        assert_eq!(back.default, d.default);
        assert_eq!(back.pin, d.pin);
    }

    #[test]
    fn unknown_lines_and_presets_are_skipped_but_a_missing_class_is_not() {
        let text = "asset-dimensions 1\nclass prop\nheight 2.1\nscale giant 40\nflavour spicy\n";
        let d = Dimensions::parse(text).unwrap();
        assert!(near(d.height, 2.1));
        assert_eq!(d.mass_kg, None, "zero-volume legacy props stay unspecified");
        assert!(Dimensions::parse("asset-dimensions 1\nheight 2.1\n").is_err());
        assert!(Dimensions::parse("stateful-billboard 1\n").is_err());
        assert!(Dimensions::parse("asset-dimensions 1\nclass prop\nheight -1\n").is_err());
    }

    #[test]
    fn legacy_sidecars_receive_class_mass_defaults() {
        let character = Dimensions::parse("asset-dimensions 1\nclass character\nheight 1.75\n").unwrap();
        let vehicle = Dimensions::parse("asset-dimensions 1\nclass vehicle\nwidth 2\nheight 1.5\nlength 4\n").unwrap();
        let prop = Dimensions::parse("asset-dimensions 1\nclass prop\nwidth 2\nheight 2\nlength 2\n").unwrap();
        assert_eq!(character.mass_kg, Some(80.0));
        assert_eq!(vehicle.mass_kg, Some(1400.0));
        assert_eq!(prop.mass_kg, Some(500.0), "volume default is capped");
        let authored = Dimensions::parse("asset-dimensions 1\nclass vehicle\nmass_kg 950\n").unwrap();
        assert_eq!(authored.mass_kg, Some(950.0));
    }

    #[test]
    fn anchors_round_trip_the_manifest_carrier() {
        let d = Dimensions::measure(SizeClass::Character, 2.5, [0.31, 0.70, 0.17], "");
        let anchors = d.anchors();
        assert_eq!(anchors.len(), 7);
        assert!(anchors.iter().all(|a| Dimensions::is_dimension_anchor(&a.name)));
        let back = Dimensions::from_anchors(AssetKind::Character, d.units_per_meter(), &anchors).unwrap();
        assert!(near(back.height, 1.75));
        assert!(near(back.radius, 0.35));
        assert!(near(back.eye, 1.65));
        for p in ScalePreset::ALL {
            assert!(near(back.factor(p), d.factor(p)), "{p:?}");
        }
        assert!(Dimensions::from_anchors(AssetKind::Mesh, 1.0, &[]).is_none());
    }

    #[test]
    fn a_size_hint_calibrates_a_unitless_generated_mesh() {
        // Trellis hands back a mesh ~1 unit tall; the expand step said 1.75 m.
        let hint = SizeHint { class: SizeClass::Character, height: 1.75, length: 0.0, width: 0.0, preset: None };
        let d = hint.measure([0.5, 1.02, 0.4], "expand: height 1.75").unwrap();
        assert!(near(d.metres_per_unit, 1.75 / 1.02));
        assert!(near(d.height, 1.75));
        assert!(near(d.eye, 1.65));
        // A vehicle pins by length and can carry its own default preset.
        let car = SizeHint { class: SizeClass::Vehicle, height: 0.0, length: 4.5, width: 0.0, preset: Some(ScalePreset::Real) };
        let d = car.measure([0.4, 0.3, 1.0], "").unwrap();
        assert!(near(d.length, 4.5));
        assert_eq!(d.default, ScalePreset::Real);
        // Nothing stated → nothing calibrated.
        let none = SizeHint { class: SizeClass::Prop, height: 0.0, length: 0.0, width: 0.0, preset: None };
        assert!(none.measure([1.0, 1.0, 1.0], "").is_none());
    }

    #[test]
    fn preset_and_class_names_round_trip() {
        for p in ScalePreset::ALL {
            assert_eq!(ScalePreset::parse(p.as_str()), Some(p));
        }
        assert_eq!(ScalePreset::parse("HandHeld"), Some(ScalePreset::Handheld));
        assert_eq!(ScalePreset::parse("giant"), None);
        assert_eq!(SizeClass::of_kind(AssetKind::Prop), SizeClass::Prop);
        assert_eq!(SizeClass::of_kind(AssetKind::Mesh), SizeClass::Prop);
        assert_eq!(SizeClass::of_kind(AssetKind::Vehicle).default_preset(), ScalePreset::Comic);
        assert_eq!(SizeClass::of_kind(AssetKind::Character).default_preset(), ScalePreset::Real);
    }
}
