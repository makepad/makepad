//! Class-derived fallback materials.
//!
//! Some interchange files arrive with no authored materials. Rather than present
//! one clay-grey building as the model's appearance,
//! [`Scene::from_model`](crate::model::Scene::from_model) synthesises a small
//! architectural palette from each element's [`ElementClass`] (with a few
//! name/layer hints) whenever the source publishes none, and records
//! [`Scene::materials_are_derived`](crate::model::Scene) so the UI can say where the
//! colours came from.
//!
//! This is a **display default, not a format claim**. When L0 lands the real
//! records, `model.materials` stops being empty and none of this runs.

use crate::model::ids::MaterialId;
use crate::model::model::{ElementClass, MaterialData};

/// A slot in the derived palette. The order here is the material order in the
/// scene, so it is also the batch order (transparent slots sort last anyway).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteSlot {
    Plaster,
    Concrete,
    Roof,
    Wood,
    Metal,
    Glass,
    Furniture,
    Lamp,
    Ground,
    Foliage,
    Zone,
    Other,
}

impl PaletteSlot {
    pub const ALL: [PaletteSlot; 12] = [
        PaletteSlot::Plaster,
        PaletteSlot::Concrete,
        PaletteSlot::Roof,
        PaletteSlot::Wood,
        PaletteSlot::Metal,
        PaletteSlot::Furniture,
        PaletteSlot::Lamp,
        PaletteSlot::Ground,
        PaletteSlot::Foliage,
        // transparent last, so the batch sort has nothing to do
        PaletteSlot::Glass,
        PaletteSlot::Zone,
        PaletteSlot::Other,
    ];

    pub fn index(self) -> usize {
        PaletteSlot::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn material(self) -> MaterialId {
        MaterialId::from_index(self.index())
    }
}

/// Every derived material, in [`PaletteSlot::ALL`] order. Colours are linear
/// RGB (roughly sRGB^2.2 of the sample colours a Fab model of this vintage
/// publishes).
pub fn palette() -> Vec<MaterialData> {
    PaletteSlot::ALL.iter().map(|s| material_for(*s)).collect()
}

fn material_for(slot: PaletteSlot) -> MaterialData {
    let base = |name: &str, rgb: [f32; 3], roughness: f32| MaterialData {
        name: format!("{name} (derived)"),
        base_color: [rgb[0], rgb[1], rgb[2], 1.0],
        roughness,
        ..Default::default()
    };
    match slot {
        PaletteSlot::Plaster => base("Plaster", [0.78, 0.77, 0.73], 0.80),
        PaletteSlot::Concrete => base("Concrete", [0.42, 0.42, 0.41], 0.85),
        PaletteSlot::Roof => base("Roof tile", [0.11, 0.06, 0.045], 0.75),
        PaletteSlot::Wood => base("Wood", [0.20, 0.10, 0.045], 0.55),
        PaletteSlot::Metal => MaterialData {
            name: "Metal (derived)".into(),
            base_color: [0.36, 0.37, 0.39, 1.0],
            metallic: 0.9,
            roughness: 0.25,
            ..Default::default()
        },
        PaletteSlot::Furniture => base("Furnishing", [0.33, 0.30, 0.26], 0.60),
        PaletteSlot::Lamp => MaterialData {
            name: "Luminaire (derived)".into(),
            base_color: [0.75, 0.72, 0.66, 1.0],
            roughness: 0.35,
            // A lamp element is the whole fixture, so this is a soft glow, not
            // a light source pretending to be photometric.
            emissive: [1.20, 1.00, 0.72],
            ..Default::default()
        },
        PaletteSlot::Ground => base("Ground", [0.13, 0.12, 0.10], 0.95),
        PaletteSlot::Foliage => base("Foliage", [0.055, 0.115, 0.035], 0.85),
        PaletteSlot::Glass => MaterialData {
            name: "Glass (derived)".into(),
            base_color: [0.78, 0.86, 0.88, 0.22],
            roughness: 0.03,
            ior: 1.52,
            transmission: 0.92,
            double_sided: true,
            ..Default::default()
        },
        PaletteSlot::Zone => MaterialData {
            // Room volumes are published as solid boxes; drawn opaque they fill
            // the building. Keep them visible but barely there.
            name: "Zone volume (derived)".into(),
            base_color: [0.25, 0.45, 0.70, 0.07],
            roughness: 0.5,
            double_sided: true,
            ..Default::default()
        },
        PaletteSlot::Other => base("Generic", [0.55, 0.54, 0.52], 0.70),
    }
}

/// Pick a slot for one element. `name` and `layer` must be lower-cased.
///
/// The layer name is the strongest signal a Fab model carries — source application
/// users name layers `Site.TreesHQ`, `Structure.Steel frames`,
/// `Covering.Roof`, `Site.Pavement`. So the hints are checked in the order a
/// person would read them (glass, greenery, ground, metal, wood) and the class
/// decides only what the names do not.
pub fn slot_for(class: &ElementClass, name: &str, layer: &str) -> PaletteSlot {
    let hint = |needles: &[&str]| needles.iter().any(|n| name.contains(n) || layer.contains(n));

    // 1. glazing — by class, and by anything that says so
    if matches!(
        class,
        ElementClass::Window | ElementClass::Skylight | ElementClass::CurtainWall
    ) || hint(&["glass", "glazing", "glaz"])
    {
        return PaletteSlot::Glass;
    }
    // 2. the two classes that are never anything else
    if *class == ElementClass::Zone {
        return PaletteSlot::Zone;
    }
    if *class == ElementClass::Lamp {
        return PaletteSlot::Lamp;
    }
    // 3. planting before ground: "Site.TreesHQ" is both a site layer and a tree
    if hint(&[
        "tree", "plant", "shrub", "grass", "bush", "hedge", "foliage", "green",
    ]) {
        return PaletteSlot::Foliage;
    }
    // 4. the site itself
    if matches!(class, ElementClass::Mesh | ElementClass::Site)
        || hint(&[
            "terrain", "ground", "soil", "asphalt", "road", "pavement", "paving", "curb",
            "landscape",
        ])
    {
        return PaletteSlot::Ground;
    }
    // 5. steelwork
    if *class == ElementClass::Railing || hint(&["steel", "metal", "alu", "brace"]) {
        return PaletteSlot::Metal;
    }
    // 6. joinery
    if *class == ElementClass::Door || hint(&["wood", "timber", "parquet"]) {
        return PaletteSlot::Wood;
    }
    // 7. context buildings read better as plaster than as furniture
    if hint(&["neighbour", "neighbor"]) {
        return PaletteSlot::Plaster;
    }
    match class {
        ElementClass::Roof => PaletteSlot::Roof,
        ElementClass::Slab | ElementClass::Column | ElementClass::Beam | ElementClass::Stair => {
            PaletteSlot::Concrete
        }
        ElementClass::Wall | ElementClass::Shell => PaletteSlot::Plaster,
        ElementClass::Furniture | ElementClass::Object | ElementClass::Morph => {
            PaletteSlot::Furniture
        }
        _ => PaletteSlot::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_slots_are_dense_and_glass_is_transparent() {
        let p = palette();
        assert_eq!(p.len(), PaletteSlot::ALL.len());
        for (i, s) in PaletteSlot::ALL.iter().enumerate() {
            assert_eq!(s.index(), i);
            assert_eq!(s.material().index(), i);
        }
        let glass = &p[PaletteSlot::Glass.index()];
        assert!(glass.base_color[3] < 1.0 && glass.transmission > 0.5);
        assert!(p[PaletteSlot::Zone.index()].base_color[3] < 0.2);
    }

    /// The layer names here are verbatim from the two samples.
    #[test]
    fn classes_and_hints_route_sensibly() {
        use ElementClass as C;
        for (class, name, layer, want) in [
            (C::Window, "wdw-001", "structure.wall", PaletteSlot::Glass),
            (C::Wall, "wall-001", "walls - exterior", PaletteSlot::Plaster),
            (C::Wall, "curtain glass panel", "", PaletteSlot::Glass),
            (C::Object, "env-021", "site.treeshq", PaletteSlot::Foliage),
            (C::Mesh, "site-001", "site.final", PaletteSlot::Ground),
            (C::Slab, "slab-002", "paving", PaletteSlot::Ground),
            (C::Wall, "wall-004", "landscape - curbs", PaletteSlot::Ground),
            (C::Beam, "bem-p-001", "structure.steel frames", PaletteSlot::Metal),
            (C::Object, "brc-001", "structure.brace", PaletteSlot::Metal),
            (C::Railing, "rlg-001", "interior.railing", PaletteSlot::Metal),
            (C::Door, "dor-001", "structure.wall partition", PaletteSlot::Wood),
            (C::Morph, "morph-001", "site.neighbours", PaletteSlot::Plaster),
            (C::Lamp, "li - 001", "interior.lights", PaletteSlot::Lamp),
            (C::Zone, "bedroom-107", "zones", PaletteSlot::Zone),
            (C::Roof, "roof-001", "covering.roof", PaletteSlot::Roof),
            (C::Column, "col-e-001", "structure.column", PaletteSlot::Concrete),
            (C::Object, "obj.-025", "furniture", PaletteSlot::Furniture),
        ] {
            assert_eq!(slot_for(&class, name, layer), want, "{name} on {layer}");
        }
    }
}
