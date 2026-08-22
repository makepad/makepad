//! The job-kind table: what a `<thing>.generate` job means on the GPU
//! fleet, and what its product becomes in the catalog.
//!
//! Every row is a triangle that must agree end to end:
//!
//! 1. an Asset Server **job kind** clients enqueue (`image.generate`),
//! 2. an asset-ai **domain** the fleet advertises in `/health` capabilities
//!    and routes models for (`image`),
//! 3. the **catalog shape** the product is published as (asset kind, file
//!    role, media type, category).
//!
//! Keeping the three in one table is what makes "wire up a domain" a data
//! change rather than a new coordinator: the profile builder reads it to
//! decide what to advertise, the claim filter reads it to decide what kinds
//! to ask for, and the publisher reads it to shape the row.
//!
//! Rows are only added for domains whose product is a REAL catalog asset.
//! `text` (the prompt expander) and `chat` are deliberately absent: their
//! output is a string an orchestrator consumes, not an asset a catalog
//! should carry.

use makepad_asset_data::{AssetKind, FileRole, MediaType};

/// What a kind needs handed to it besides a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputNeed {
    /// Prompt-only: the fleet generates from text alone.
    None,
    /// Requires a source image (edit/upscale/depth/matte/control/…).
    Image,
    /// Requires a source mesh GLB (paint/rig/motion).
    Mesh,
    /// Requires a source video mp4 (enhance).
    Video,
}

impl InputNeed {
    /// The content type the fleet is told the relayed input carries.
    pub fn content_type(self) -> &'static str {
        match self {
            InputNeed::Mesh => "model/gltf-binary",
            InputNeed::Video => "video/mp4",
            _ => "image/png",
        }
    }
}

/// One wired generation kind.
#[derive(Clone, Copy, Debug)]
pub struct GenKind {
    /// Asset Server job kind (the client-facing contract).
    pub kind: &'static str,
    /// asset-ai capability domain (what `/health` advertises and `/models`
    /// groups by).
    pub domain: &'static str,
    /// Published catalog kind.
    pub asset_kind: AssetKind,
    pub role: FileRole,
    pub media: MediaType,
    /// Search category of the published row.
    pub category: &'static str,
    /// Extra tags every product of this kind carries (scope markers the
    /// consumers filter on, e.g. object vs world splats).
    pub tags: &'static [&'static str],
    /// Artifact content types the fleet may return for this kind. The first
    /// is the expected one; the rest are accepted aliases of the same
    /// payload class. Anything else is a hard failure — never guessed.
    pub content_types: &'static [&'static str],
    pub input: InputNeed,
    /// Human label prefix for advertised profiles ("FLUX.1 schnell" style
    /// text is per model; this names the ACTION).
    pub action: &'static str,
}

const PNG: &[&str] = &["image/png"];
const WAV: &[&str] = &["audio/wav"];
const GLB: &[&str] = &["model/gltf-binary"];
const PLY: &[&str] = &["application/x-ply", "application/octet-stream"];
const MP4: &[&str] = &["video/mp4"];

/// Every wired kind, in advertisement order (cheap/most used first).
pub const GEN_KINDS: &[GenKind] = &[
    GenKind {
        kind: "image.generate",
        domain: "image",
        asset_kind: AssetKind::Texture,
        role: FileRole::Texture,
        media: MediaType::Png,
        category: "image",
        tags: &[],
        content_types: PNG,
        input: InputNeed::None,
        action: "image",
    },
    GenKind {
        kind: "image.edit",
        domain: "edit",
        asset_kind: AssetKind::Texture,
        role: FileRole::Texture,
        media: MediaType::Png,
        category: "image",
        tags: &["edit"],
        content_types: PNG,
        input: InputNeed::Image,
        action: "edit image",
    },
    GenKind {
        kind: "image.upscale",
        domain: "upscale",
        asset_kind: AssetKind::Texture,
        role: FileRole::Texture,
        media: MediaType::Png,
        category: "image",
        tags: &["upscaled"],
        content_types: PNG,
        input: InputNeed::Image,
        action: "upscale image",
    },
    GenKind {
        kind: "image.inpaint",
        domain: "inpaint",
        asset_kind: AssetKind::Texture,
        role: FileRole::Texture,
        media: MediaType::Png,
        category: "image",
        tags: &["inpaint"],
        content_types: PNG,
        input: InputNeed::Image,
        action: "inpaint image",
    },
    GenKind {
        kind: "image.control",
        domain: "control",
        asset_kind: AssetKind::Texture,
        role: FileRole::Texture,
        media: MediaType::Png,
        category: "image",
        tags: &["control"],
        content_types: PNG,
        input: InputNeed::Image,
        action: "structure-guided image",
    },
    GenKind {
        kind: "image.matte",
        domain: "matte",
        asset_kind: AssetKind::Texture,
        role: FileRole::Texture,
        media: MediaType::Png,
        category: "image",
        tags: &["matte"],
        content_types: PNG,
        input: InputNeed::Image,
        action: "cutout",
    },
    GenKind {
        // 16-bit metric depth: its own file role, never a plain texture —
        // a consumer that samples millimetres must be able to tell.
        kind: "image.depth",
        domain: "depth",
        asset_kind: AssetKind::Texture,
        role: FileRole::Depth,
        media: MediaType::Png,
        category: "image",
        tags: &["depth"],
        content_types: PNG,
        input: InputNeed::Image,
        action: "metric depth",
    },
    GenKind {
        kind: "video.generate",
        domain: "video",
        asset_kind: AssetKind::Video,
        role: FileRole::Video,
        media: MediaType::Mp4,
        category: "generated",
        tags: &[],
        content_types: MP4,
        input: InputNeed::None,
        action: "video",
    },
    GenKind {
        kind: "video.enhance",
        domain: "enhance",
        asset_kind: AssetKind::Video,
        role: FileRole::Video,
        media: MediaType::Mp4,
        category: "generated",
        tags: &["enhanced"],
        content_types: MP4,
        input: InputNeed::Video,
        action: "enhance video",
    },
    GenKind {
        kind: "audio.generate",
        domain: "audio",
        asset_kind: AssetKind::Audio,
        role: FileRole::Audio,
        media: MediaType::Wav,
        category: "sfx",
        tags: &[],
        content_types: WAV,
        input: InputNeed::None,
        action: "sound effect",
    },
    GenKind {
        kind: "music.generate",
        domain: "music",
        asset_kind: AssetKind::Audio,
        role: FileRole::Audio,
        media: MediaType::Wav,
        // The VJ keeps long-form tracks and one-shot pads apart by
        // category, exactly as the library importer does.
        category: "music",
        tags: &[],
        content_types: WAV,
        input: InputNeed::None,
        action: "music",
    },
    GenKind {
        kind: "speech.generate",
        domain: "speech",
        asset_kind: AssetKind::Audio,
        role: FileRole::Audio,
        media: MediaType::Wav,
        category: "speech",
        tags: &[],
        content_types: WAV,
        input: InputNeed::None,
        action: "speech",
    },
    GenKind {
        kind: "mesh.generate",
        domain: "mesh",
        asset_kind: AssetKind::Mesh,
        role: FileRole::RenderGlb,
        media: MediaType::Glb,
        category: "prop",
        tags: &[],
        content_types: GLB,
        input: InputNeed::Image,
        action: "3D model",
    },
    GenKind {
        kind: "mesh.paint",
        domain: "paint",
        asset_kind: AssetKind::Mesh,
        role: FileRole::RenderGlb,
        media: MediaType::Glb,
        category: "prop",
        tags: &["pbr"],
        content_types: GLB,
        input: InputNeed::Mesh,
        action: "PBR texturing",
    },
    GenKind {
        kind: "mesh.rig",
        domain: "rig",
        asset_kind: AssetKind::Character,
        role: FileRole::RenderGlb,
        media: MediaType::Glb,
        category: "dancer",
        tags: &["rigged"],
        content_types: GLB,
        input: InputNeed::Mesh,
        action: "rig",
    },
    GenKind {
        kind: "mesh.motion",
        domain: "motion",
        asset_kind: AssetKind::Character,
        role: FileRole::RenderGlb,
        media: MediaType::Glb,
        category: "dancer",
        tags: &["animated"],
        content_types: GLB,
        input: InputNeed::Mesh,
        action: "motion",
    },
    GenKind {
        // A splat scene IS a world, distinguished by category + file role
        // (same law the ai-library importer follows). `object` scope keeps
        // one-object splats apart from walkable scenes.
        kind: "splat.generate",
        domain: "splat",
        asset_kind: AssetKind::World,
        role: FileRole::Splat,
        media: MediaType::Ply,
        category: "splat",
        tags: &["object"],
        content_types: PLY,
        input: InputNeed::Image,
        action: "object splat",
    },
    GenKind {
        kind: "world.generate",
        domain: "world",
        asset_kind: AssetKind::World,
        role: FileRole::Splat,
        media: MediaType::Ply,
        category: "splat",
        tags: &["world"],
        content_types: PLY,
        input: InputNeed::None,
        action: "walkable world",
    },
];

/// The wired kind for a job kind, if any.
pub fn kind_of(job_kind: &str) -> Option<&'static GenKind> {
    GEN_KINDS.iter().find(|k| k.kind == job_kind)
}

/// The wired kind for an asset-ai domain, if any.
pub fn kind_for_domain(domain: &str) -> Option<&'static GenKind> {
    GEN_KINDS.iter().find(|k| k.domain == domain)
}

/// Every job kind wired for the domains a box advertises. The list is
/// deduplicated and capped at the server's 32-kind claim limit, so a box
/// that grows new capabilities can never make the claim call refuse.
pub fn kinds_for_domains(domains: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for row in GEN_KINDS {
        if domains.iter().any(|d| d == row.domain) && !out.iter().any(|k| k == row.kind) {
            out.push(row.kind.to_string());
        }
        if out.len() == 32 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_internally_consistent() {
        for row in GEN_KINDS {
            assert!(
                row.role.allows(row.media),
                "{}: role {:?} cannot carry {:?}",
                row.kind,
                row.role,
                row.media
            );
            assert!(!row.content_types.is_empty(), "{}: no content type", row.kind);
            assert_eq!(
                GEN_KINDS.iter().filter(|o| o.kind == row.kind).count(),
                1,
                "{}: duplicate kind",
                row.kind
            );
            assert_eq!(
                GEN_KINDS.iter().filter(|o| o.domain == row.domain).count(),
                1,
                "{}: duplicate domain",
                row.domain
            );
            // Job kinds must survive the server's kind validation charset.
            assert!(row
                .kind
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b == b'.' || b == b'_'));
        }
    }

    #[test]
    fn domains_map_to_claimable_kind_filters() {
        let domains: Vec<String> = ["image", "video", "chat", "text"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            kinds_for_domains(&domains),
            vec!["image.generate".to_string(), "video.generate".to_string()]
        );
        // A box with no wired domain claims nothing at all rather than
        // claiming everything.
        assert!(kinds_for_domains(&["chat".to_string()]).is_empty());
        // Every wired domain at once still fits the server's claim cap.
        let all: Vec<String> = GEN_KINDS.iter().map(|k| k.domain.to_string()).collect();
        assert_eq!(kinds_for_domains(&all).len(), GEN_KINDS.len());
        assert!(GEN_KINDS.len() <= 32);
    }

    #[test]
    fn lookups_agree_with_the_table() {
        assert_eq!(kind_of("image.generate").unwrap().domain, "image");
        assert_eq!(kind_for_domain("music").unwrap().kind, "music.generate");
        assert!(kind_of("text.expand").is_none());
        assert!(kind_for_domain("chat").is_none());
        assert_eq!(InputNeed::Mesh.content_type(), "model/gltf-binary");
        assert_eq!(InputNeed::None.content_type(), "image/png");
    }
}
