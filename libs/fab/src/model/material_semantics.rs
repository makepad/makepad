//! Material meaning recovered from legacy container material records.
//!
//! Alpha alone is not a glass flag: vegetation, fabrics, decals and room
//! volumes can all be transparent. This pass is deliberately conservative
//! and promotes only material names that identify glazing (with alpha as a
//! secondary signal for otherwise generic window materials).

use crate::model::MaterialData;

/// Apply architectural semantics to one decoded legacy material.
pub fn apply_legacy_material_semantics(material: &mut MaterialData) {
    if !is_glazing(&material.name, material.base_color[3]) {
        return;
    }
    let alpha_transmission = (1.0 - material.base_color[3]).clamp(0.0, 1.0);
    material.transmission = material.transmission.max(if alpha_transmission > 0.01 {
        alpha_transmission
    } else {
        0.92
    });
    material.ior = 1.52;
    material.roughness = material.roughness.min(0.02);
    material.double_sided = true;
}

/// Whether a legacy material record denotes a window pane rather than a
/// merely alpha-blended surface.
pub fn is_glazing(name: &str, alpha: f32) -> bool {
    let name = name.to_ascii_lowercase();
    let explicit_glass = ["glass", "glazing", "glazed", "glaz", "window pane", "door lite"]
        .iter()
        .any(|term| name.contains(term));
    if explicit_glass {
        return true;
    }

    let window_material = name.contains("window") || name.contains("fenestration");
    let frame_part = ["frame", "sash", "trim", "handle", "hardware", "jamb"]
        .iter()
        .any(|term| name.contains(term));
    window_material && !frame_part && alpha < 0.99
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(name: &str, alpha: f32) -> MaterialData {
        MaterialData {
            name: name.into(),
            base_color: [0.8, 0.9, 1.0, alpha],
            roughness: 0.7,
            ..Default::default()
        }
    }

    #[test]
    fn named_glass_becomes_thin_dielectric() {
        let mut clear = material("Glass: Clear, Fast*0", 0.31);
        apply_legacy_material_semantics(&mut clear);
        assert!((clear.transmission - 0.69).abs() < 1.0e-6);
        assert_eq!(clear.ior, 1.52);
        assert_eq!(clear.roughness, 0.02);
        assert!(clear.double_sided);

        let mut opaque_record = material("Door with glass", 1.0);
        apply_legacy_material_semantics(&mut opaque_record);
        assert_eq!(opaque_record.transmission, 0.92);
    }

    #[test]
    fn alpha_and_window_frames_are_not_glass() {
        for (name, alpha) in [("Curtain fabric", 0.3), ("Foliage", 0.5), ("Window frame", 0.4)] {
            let mut candidate = material(name, alpha);
            apply_legacy_material_semantics(&mut candidate);
            assert_eq!(candidate.transmission, 0.0, "{name}");
        }
        assert!(is_glazing("Window pane", 0.7));
        assert!(is_glazing("Window generic", 0.7));
    }
}
