//! Materials and impact energy: turning a physics contact into a sound choice.
//!
//! The engine knows when two things hit and how hard. That is enough to pick
//! a sound and set its gain and pitch, which is why games built on blocks are
//! audible without the AI wiring a single cue.

/// What a thing is made of. Defaults to `Plastic` because Kenney's palette is
/// toy-like and a wrong-but-neutral knock beats silence.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Material {
    Wood,
    Metal,
    Stone,
    Dirt,
    Glass,
    #[default]
    Plastic,
}

impl Material {
    pub fn name(self) -> &'static str {
        match self {
            Material::Wood => "wood",
            Material::Metal => "metal",
            Material::Stone => "stone",
            Material::Dirt => "dirt",
            Material::Glass => "glass",
            Material::Plastic => "plastic",
        }
    }

    pub fn parse(s: &str) -> Option<Material> {
        Some(match s {
            "wood" | "timber" => Material::Wood,
            "metal" | "steel" | "iron" => Material::Metal,
            "stone" | "rock" | "concrete" => Material::Stone,
            "dirt" | "earth" | "grass" | "sand" => Material::Dirt,
            "glass" => Material::Glass,
            "plastic" | "rubber" => Material::Plastic,
            _ => return None,
        })
    }

    /// How bright the material rings; scales pitch so a small metal box does
    /// not sound like a boulder.
    fn brightness(self) -> f32 {
        match self {
            Material::Glass => 1.35,
            Material::Metal => 1.2,
            Material::Wood => 1.0,
            Material::Plastic => 1.05,
            Material::Stone => 0.85,
            Material::Dirt => 0.75,
        }
    }

    /// Relative loudness: dirt absorbs, glass and metal do not.
    fn resonance(self) -> f32 {
        match self {
            Material::Glass => 1.0,
            Material::Metal => 1.0,
            Material::Stone => 0.85,
            Material::Wood => 0.8,
            Material::Plastic => 0.7,
            Material::Dirt => 0.45,
        }
    }
}

/// An unordered pair of materials — wood hitting metal is the same event as
/// metal hitting wood, and the sound table should not need both entries.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MaterialPair(pub Material, pub Material);

impl MaterialPair {
    pub fn new(a: Material, b: Material) -> Self {
        // Canonical order so lookups and cooldowns agree.
        if (a as u8) <= (b as u8) {
            MaterialPair(a, b)
        } else {
            MaterialPair(b, a)
        }
    }

    /// The category key a sound pack is indexed under. The softer material
    /// dominates: a plank landing on dirt thuds, it does not clack.
    pub fn key(self) -> &'static str {
        let (a, b) = (self.0, self.1);
        let softer = if a.resonance() <= b.resonance() { a } else { b };
        softer.name()
    }
}

/// How an impact maps to gain and pitch.
///
/// Speed rather than energy: mass is not always meaningful for kinematic
/// bodies, and players judge loudness by how fast something was moving.
#[derive(Clone, Copy, Debug)]
pub struct ImpactCurve {
    /// Below this the impact is inaudible — stops resting bodies chattering.
    pub min_speed: f32,
    /// At and above this the sound is at full volume.
    pub full_speed: f32,
    /// Pitch variation across the speed range: fast hits ring higher.
    pub pitch_range: f32,
}

impl Default for ImpactCurve {
    fn default() -> Self {
        Self {
            min_speed: 0.8,
            full_speed: 9.0,
            pitch_range: 0.35,
        }
    }
}

/// The sound parameters for one contact, or `None` if it is too gentle to hear.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ImpactSound {
    pub gain: f32,
    pub pitch: f32,
}

impl ImpactCurve {
    pub fn evaluate(&self, speed: f32, pair: MaterialPair) -> Option<ImpactSound> {
        if !speed.is_finite() || speed < self.min_speed {
            return None;
        }
        let span = (self.full_speed - self.min_speed).max(0.001);
        let t = ((speed - self.min_speed) / span).clamp(0.0, 1.0);
        // Square-root keeps quiet hits audible; linear makes everything soft
        // until it is suddenly loud.
        let loudness = t.sqrt();
        let resonance = pair.0.resonance().max(pair.1.resonance());
        let brightness = (pair.0.brightness() + pair.1.brightness()) * 0.5;
        Some(ImpactSound {
            gain: (loudness * resonance).clamp(0.0, 1.0),
            // Faster hits pitch up a little, within the material's character.
            pitch: (brightness * (1.0 - self.pitch_range * 0.5 + self.pitch_range * t))
                .clamp(0.25, 4.0),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_pairs_are_order_independent() {
        let a = MaterialPair::new(Material::Wood, Material::Metal);
        let b = MaterialPair::new(Material::Metal, Material::Wood);
        assert_eq!(a, b);
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn the_softer_material_names_the_sound() {
        // Dirt absorbs more than metal, so a metal-on-dirt hit is a thud.
        let p = MaterialPair::new(Material::Metal, Material::Dirt);
        assert_eq!(p.key(), "dirt");
        let q = MaterialPair::new(Material::Glass, Material::Metal);
        // Equal resonance: still deterministic, never a coin flip.
        assert_eq!(q.key(), MaterialPair::new(Material::Metal, Material::Glass).key());
    }

    #[test]
    fn gentle_contacts_make_no_sound() {
        let c = ImpactCurve::default();
        let pair = MaterialPair::new(Material::Wood, Material::Wood);
        assert!(c.evaluate(0.0, pair).is_none());
        assert!(c.evaluate(0.5, pair).is_none());
        assert!(c.evaluate(f32::NAN, pair).is_none());
        assert!(c.evaluate(2.0, pair).is_some());
    }

    #[test]
    fn louder_with_speed_up_to_a_ceiling() {
        let c = ImpactCurve::default();
        let pair = MaterialPair::new(Material::Wood, Material::Wood);
        let slow = c.evaluate(1.5, pair).unwrap();
        let fast = c.evaluate(8.0, pair).unwrap();
        let absurd = c.evaluate(500.0, pair).unwrap();
        assert!(fast.gain > slow.gain);
        assert!(absurd.gain <= 1.0);
        assert!(fast.pitch > slow.pitch, "fast hits should ring higher");
    }

    #[test]
    fn dirt_is_quieter_than_glass_at_the_same_speed() {
        let c = ImpactCurve::default();
        let soft = c
            .evaluate(6.0, MaterialPair::new(Material::Dirt, Material::Dirt))
            .unwrap();
        let hard = c
            .evaluate(6.0, MaterialPair::new(Material::Glass, Material::Glass))
            .unwrap();
        assert!(hard.gain > soft.gain);
        assert!(hard.pitch > soft.pitch);
    }

    #[test]
    fn parsing_is_forgiving_but_bounded() {
        assert_eq!(Material::parse("rock"), Some(Material::Stone));
        assert_eq!(Material::parse("timber"), Some(Material::Wood));
        assert_eq!(Material::parse("cheese"), None);
        assert_eq!(Material::default(), Material::Plastic);
    }
}
