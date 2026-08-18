//! Post-process pass: walk the local library and fill missing metadata.
//!
//! Two layers, both fail-closed:
//! 1. Catalog names (Doom lump → Imp, Quake MDL → Shambler). No model.
//! 2. Optional vision caption into `<file>.vision.json` when a Qwen-VL
//!    (or any fleet model with `vl` in the id) is actually ready. Until
//!    that box is provisioned the pass records `skipped` and does not
//!    invent descriptions.

use crate::library::{Library, LibraryMeta};
use makepad_asset_importer::stateful_billboard::{mesh_title, sprite_title, world_title};
use std::path::{Path, PathBuf};

pub const VISION_SIDECAR: &str = "vision.json";

#[derive(Clone, Debug, Default)]
pub struct EnhanceStats {
    pub scanned: usize,
    pub named: usize,
    pub vision_wrote: usize,
    pub vision_skipped: usize,
    pub message: String,
}

pub fn sidecar_path(library_dir: &Path, file: &str) -> PathBuf {
    library_dir.join(format!("{file}.{VISION_SIDECAR}"))
}

pub fn has_vision(library_dir: &Path, file: &str) -> bool {
    sidecar_path(library_dir, file).is_file()
}

/// Rewrite path-like labels to catalog titles. Does not invent lore.
pub fn apply_catalog_names(library: &mut Library) -> usize {
    let mut changed = 0usize;
    for item in &mut library.index.items {
        if let Some(next) = better_label(item) {
            if next != item.label {
                item.label = next;
                changed += 1;
            }
        }
    }
    if changed > 0 {
        let _ = library.persist();
    }
    changed
}

fn better_label(item: &LibraryMeta) -> Option<String> {
    let domain = item.domain.as_str();
    let stem = prompt_asset_stem(&item.prompt)
        .or_else(|| label_asset_stem(&item.label))
        .unwrap_or(item.label.as_str());
    let title = match domain {
        "billboard" => sprite_title(stem),
        "world" => world_title(stem),
        "character" | "weapon" | "prop" => mesh_title(stem),
        _ => return None,
    };
    if title.is_empty() || title == item.label {
        None
    } else {
        Some(title)
    }
}

fn is_meta_segment(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    if l.is_empty() {
        return true;
    }
    if l.starts_with("http://") || l.starts_with("https://") {
        return true;
    }
    if l.contains("cc0")
        || l.contains("cc-by")
        || l.contains("cc by")
        || l.contains("public domain")
        || l.contains("shareware")
        || l.contains("license")
    {
        return true;
    }
    matches!(
        l.as_str(),
        "character"
            | "world"
            | "billboard"
            | "weapon"
            | "prop"
            | "audio"
            | "music"
            | "texture"
            | "image"
            | "video"
    )
}

/// Classic prompts are `source · kind · stem · license · url`. Kenney is
/// `source · stem · license · url`. Never take the license as the title.
fn prompt_asset_stem(prompt: &str) -> Option<&str> {
    let segs: Vec<&str> = prompt
        .split('·')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !is_meta_segment(s))
        .collect();
    segs.iter()
        .rev()
        .find(|s| !s.contains(' '))
        .copied()
        .or_else(|| {
            segs.first().and_then(|s| {
                s.rsplit([' ', '/'])
                    .map(str::trim)
                    .find(|w| !w.is_empty() && !is_meta_segment(w))
            })
        })
}

fn label_asset_stem(label: &str) -> Option<&str> {
    let stem = label.rsplit('/').next().unwrap_or(label).trim();
    if stem.is_empty() || is_meta_segment(stem) {
        None
    } else {
        Some(stem)
    }
}

/// Mark assets that have no vision blob. Does not call a model unless
/// `vision_ready` is true — then the caller supplies captions.
pub fn stamp_skipped_vision(library_dir: &Path, items: &[LibraryMeta], reason: &str) -> usize {
    let mut n = 0usize;
    for item in items {
        let path = sidecar_path(library_dir, &item.file);
        if path.is_file() {
            continue;
        }
        let body = format!(
            "{{\"status\":\"skipped\",\"reason\":\"{}\",\"label\":\"{}\",\"domain\":\"{}\"}}\n",
            reason.replace('"', "'"),
            item.label.replace('"', "'"),
            item.domain
        );
        if std::fs::write(&path, body).is_ok() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(domain: &str, label: &str, prompt: &str) -> LibraryMeta {
        LibraryMeta {
            file: "lib-1.glb".into(),
            label: label.into(),
            domain: domain.into(),
            content_type: "model/gltf-binary".into(),
            prompt: prompt.into(),
            group_id: None,
            group_label: None,
            tags: None,
            enhanced_tags: None,
        }
    }

    #[test]
    fn kaykit_prompt_does_not_become_the_license() {
        let broken = item(
            "character",
            "Cc0 1.0",
            "KayKit knight · character · CC0-1.0 · https://kaylousberg.com/",
        );
        assert_eq!(better_label(&broken).as_deref(), Some("Knight"));
        let pathy = item(
            "character",
            "kaykit/adventurers/barbarian",
            "KayKit adventurers · character · barbarian · CC0-1.0 · https://kaylousberg.com/",
        );
        assert_eq!(better_label(&pathy).as_deref(), Some("Barbarian"));
    }

    #[test]
    fn kenney_and_classic_stems_stay_the_asset() {
        let kenney = item(
            "character",
            "kenney/characters/rogue",
            "Kenney characters · rogue · CC-BY-4.0 · https://kenney.nl/",
        );
        assert_eq!(better_label(&kenney).as_deref(), Some("Rogue"));
        let classic = item(
            "character",
            "imp",
            "Freedoom freedoom2 · character · imp · CC0-1.0 · https://freedoom.github.io/",
        );
        assert_eq!(better_label(&classic).as_deref(), Some("Imp"));
        let duke = item(
            "billboard",
            "tile-0123",
            "Duke Nukem 3D (shareware) duke3d · billboard · tile-0123 · 3D-Realms-shareware · https://archive.org/",
        );
        assert_eq!(better_label(&duke).as_deref(), Some("TILE-0123"));
    }
}

pub fn fleet_has_vision(model_ids: &[String]) -> bool {
    model_ids.iter().any(|id| {
        let l = id.to_ascii_lowercase();
        l.contains("vl") || l.contains("vision") || l.contains("qwen2.5-vl") || l.contains("qwen3-vl")
    })
}
