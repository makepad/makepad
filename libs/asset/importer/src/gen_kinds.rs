//! The job-kind table: what a `<thing>.generate` job means on the GPU
//! fleet, and what its product becomes.
//!
//! Every row is a triangle that must agree end to end:
//!
//! 1. an Asset Server **job kind** clients enqueue (`image.generate`),
//! 2. an asset-ai **domain** the fleet advertises in `/health` capabilities
//!    and routes models for (`image`),
//! 3. the **product** — what the answer becomes when it comes back.
//!
//! Keeping the three in one table is what makes "wire up a domain" a data
//! change rather than a new coordinator: the profile builder reads it to
//! decide what to advertise, the claim filter reads it to decide what kinds
//! to ask for, and the coordinator reads it to decide what to do with the
//! answer.
//!
//! Most rows publish a REAL catalog asset ([`Product::Catalog`]). Two do
//! not, and they are why the product is a typed decision rather than an
//! assumption: a `vision` box answers a QUESTION about an image, and an
//! answer is not an asset. `vision.describe` records its answer on the job
//! for whoever asked; `annotate.asset` folds it into an existing asset's
//! annotation record. `text` (the prompt expander) and `chat` remain absent
//! entirely: nothing in this worker executes them.

use makepad_asset_data::{AssetKind, FileRole, MediaType};

/// What a kind needs handed to it besides a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputNeed {
    /// Prompt-only: the fleet generates from text alone.
    None,
    /// Requires a source image (edit/upscale/depth/matte/control/vision/…).
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

/// The catalog shape a published product takes.
#[derive(Clone, Copy, Debug)]
pub struct CatalogShape {
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
}

/// What a finished job leaves behind. The coordinator branches on exactly
/// this: there is no other place where "does this publish?" is decided.
#[derive(Clone, Copy, Debug)]
pub enum Product {
    /// One published catalog row of this shape (artifact + thumbnail).
    Catalog(CatalogShape),
    /// TEXT recorded on the job (`worker_succeed` result `{text, model,
    /// box}`), nothing published. Any client reads it back from
    /// `GET /v1/jobs/<id>` — that is the runtime path for a UI that needs
    /// an answer about an image while it is making content.
    Text,
    /// The answer is folded into an EXISTING asset's annotation record
    /// (description + `vlm-` facets). Nothing is published and nothing is
    /// created; the job records what it wrote.
    Annotation,
}

/// One wired generation kind.
#[derive(Clone, Copy, Debug)]
pub struct GenKind {
    /// Asset Server job kind (the client-facing contract).
    pub kind: &'static str,
    /// asset-ai capability domain (what `/health` advertises and `/models`
    /// groups by).
    pub domain: &'static str,
    /// What the answer becomes.
    pub product: Product,
    pub input: InputNeed,
    /// Human label prefix for advertised profiles ("FLUX.1 schnell" style
    /// text is per model; this names the ACTION).
    pub action: &'static str,
}

impl GenKind {
    /// The catalog shape this kind publishes, or `None` when its answer is
    /// not a catalog row.
    pub fn catalog(&self) -> Option<&CatalogShape> {
        match &self.product {
            Product::Catalog(shape) => Some(shape),
            _ => None,
        }
    }

    /// True when the answer is text recorded on the job.
    pub fn is_text(&self) -> bool {
        matches!(self.product, Product::Text)
    }

    /// True when the answer is folded into an existing asset's annotation.
    pub fn is_annotation(&self) -> bool {
        matches!(self.product, Product::Annotation)
    }

    /// True when a client picks this kind from an advertised profile.
    ///
    /// An annotation job is minted BY the server for an asset that already
    /// exists; there is nothing for a picker to choose and no prompt for a
    /// human to write, so it is never advertised.
    pub fn advertised(&self) -> bool {
        !self.is_annotation()
    }
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
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Texture,
            role: FileRole::Texture,
            media: MediaType::Png,
            category: "image",
            tags: &[],
            content_types: PNG,
        }),
        input: InputNeed::None,
        action: "image",
    },
    GenKind {
        kind: "image.edit",
        domain: "edit",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Texture,
            role: FileRole::Texture,
            media: MediaType::Png,
            category: "image",
            tags: &["edit"],
            content_types: PNG,
        }),
        input: InputNeed::Image,
        action: "edit image",
    },
    GenKind {
        kind: "image.upscale",
        domain: "upscale",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Texture,
            role: FileRole::Texture,
            media: MediaType::Png,
            category: "image",
            tags: &["upscaled"],
            content_types: PNG,
        }),
        input: InputNeed::Image,
        action: "upscale image",
    },
    GenKind {
        kind: "image.inpaint",
        domain: "inpaint",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Texture,
            role: FileRole::Texture,
            media: MediaType::Png,
            category: "image",
            tags: &["inpaint"],
            content_types: PNG,
        }),
        input: InputNeed::Image,
        action: "inpaint image",
    },
    GenKind {
        kind: "image.control",
        domain: "control",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Texture,
            role: FileRole::Texture,
            media: MediaType::Png,
            category: "image",
            tags: &["control"],
            content_types: PNG,
        }),
        input: InputNeed::Image,
        action: "structure-guided image",
    },
    GenKind {
        kind: "image.matte",
        domain: "matte",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Texture,
            role: FileRole::Texture,
            media: MediaType::Png,
            category: "image",
            tags: &["matte"],
            content_types: PNG,
        }),
        input: InputNeed::Image,
        action: "cutout",
    },
    GenKind {
        // 16-bit metric depth: its own file role, never a plain texture —
        // a consumer that samples millimetres must be able to tell.
        kind: "image.depth",
        domain: "depth",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Texture,
            role: FileRole::Depth,
            media: MediaType::Png,
            category: "image",
            tags: &["depth"],
            content_types: PNG,
        }),
        input: InputNeed::Image,
        action: "metric depth",
    },
    GenKind {
        // A question about an image, answered as text on the JOB. A UI
        // making content asks it at runtime and reads `result.text` back;
        // nothing lands in the catalog, because an answer is not an asset.
        kind: "vision.describe",
        domain: "vision",
        product: Product::Text,
        input: InputNeed::Image,
        action: "answer about an image",
    },
    GenKind {
        // The catalog's own annotation pass, executed as a fleet job: the
        // server mints one per newly live annotatable asset
        // (`makepad_asset_store::host::annotate`), a vision box answers it,
        // and the parsed record replaces that asset's description and
        // `vlm-` facets. Same domain as `vision.describe` — one capability,
        // two kinds — and never advertised: the server queues it, nobody
        // picks it.
        kind: "annotate.asset",
        domain: "vision",
        product: Product::Annotation,
        input: InputNeed::Image,
        action: "describe a catalog asset",
    },
    GenKind {
        kind: "video.generate",
        domain: "video",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Video,
            role: FileRole::Video,
            media: MediaType::Mp4,
            category: "generated",
            tags: &[],
            content_types: MP4,
        }),
        input: InputNeed::None,
        action: "video",
    },
    GenKind {
        kind: "video.enhance",
        domain: "enhance",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Video,
            role: FileRole::Video,
            media: MediaType::Mp4,
            category: "generated",
            tags: &["enhanced"],
            content_types: MP4,
        }),
        input: InputNeed::Video,
        action: "enhance video",
    },
    GenKind {
        kind: "audio.generate",
        domain: "audio",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Audio,
            role: FileRole::Audio,
            media: MediaType::Wav,
            category: "sfx",
            tags: &[],
            content_types: WAV,
        }),
        input: InputNeed::None,
        action: "sound effect",
    },
    GenKind {
        kind: "music.generate",
        domain: "music",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Audio,
            role: FileRole::Audio,
            media: MediaType::Wav,
            // The VJ keeps long-form tracks and one-shot pads apart by
            // category, exactly as the library importer does.
            category: "music",
            tags: &[],
            content_types: WAV,
        }),
        input: InputNeed::None,
        action: "music",
    },
    GenKind {
        kind: "speech.generate",
        domain: "speech",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Audio,
            role: FileRole::Audio,
            media: MediaType::Wav,
            category: "speech",
            tags: &[],
            content_types: WAV,
        }),
        input: InputNeed::None,
        action: "speech",
    },
    GenKind {
        kind: "mesh.generate",
        domain: "mesh",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Mesh,
            role: FileRole::RenderGlb,
            media: MediaType::Glb,
            category: "prop",
            tags: &[],
            content_types: GLB,
        }),
        input: InputNeed::Image,
        action: "3D model",
    },
    GenKind {
        kind: "mesh.paint",
        domain: "paint",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Mesh,
            role: FileRole::RenderGlb,
            media: MediaType::Glb,
            category: "prop",
            tags: &["pbr"],
            content_types: GLB,
        }),
        input: InputNeed::Mesh,
        action: "PBR texturing",
    },
    GenKind {
        kind: "mesh.rig",
        domain: "rig",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Character,
            role: FileRole::RenderGlb,
            media: MediaType::Glb,
            category: "dancer",
            tags: &["rigged"],
            content_types: GLB,
        }),
        input: InputNeed::Mesh,
        action: "rig",
    },
    GenKind {
        kind: "mesh.motion",
        domain: "motion",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::Character,
            role: FileRole::RenderGlb,
            media: MediaType::Glb,
            category: "dancer",
            tags: &["animated"],
            content_types: GLB,
        }),
        input: InputNeed::Mesh,
        action: "motion",
    },
    GenKind {
        // A splat scene IS a world, distinguished by category + file role
        // (same law the ai-library importer follows). `object` scope keeps
        // one-object splats apart from walkable scenes.
        kind: "splat.generate",
        domain: "splat",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::World,
            role: FileRole::Splat,
            media: MediaType::Ply,
            category: "splat",
            tags: &["object"],
            content_types: PLY,
        }),
        input: InputNeed::Image,
        action: "object splat",
    },
    GenKind {
        kind: "world.generate",
        domain: "world",
        product: Product::Catalog(CatalogShape {
            asset_kind: AssetKind::World,
            role: FileRole::Splat,
            media: MediaType::Ply,
            category: "splat",
            tags: &["world"],
            content_types: PLY,
        }),
        input: InputNeed::None,
        action: "walkable world",
    },
];

/// The wired kind for a job kind, if any.
pub fn kind_of(job_kind: &str) -> Option<&'static GenKind> {
    GEN_KINDS.iter().find(|k| k.kind == job_kind)
}

/// The kind a domain ADVERTISES, if any. A domain can carry more than one
/// kind (`vision` carries the client-facing question and the catalog's own
/// annotation pass); only one of them is ever offered to a picker.
pub fn kind_for_domain(domain: &str) -> Option<&'static GenKind> {
    GEN_KINDS
        .iter()
        .find(|k| k.domain == domain && k.advertised())
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
            if let Some(shape) = row.catalog() {
                assert!(
                    shape.role.allows(shape.media),
                    "{}: role {:?} cannot carry {:?}",
                    row.kind,
                    shape.role,
                    shape.media
                );
                assert!(!shape.content_types.is_empty(), "{}: no content type", row.kind);
            }
            assert_eq!(
                GEN_KINDS.iter().filter(|o| o.kind == row.kind).count(),
                1,
                "{}: duplicate kind",
                row.kind
            );
            // A domain may carry several kinds, but only ONE of them is
            // advertised — two profiles for one capability would put the
            // same GPU behind two picker entries that mean the same thing.
            assert_eq!(
                GEN_KINDS
                    .iter()
                    .filter(|o| o.domain == row.domain && o.advertised())
                    .count(),
                1,
                "{}: duplicate advertised domain",
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
        // One capability, both of its kinds: a vision box claims the
        // client's questions AND the catalog's annotation backlog.
        assert_eq!(
            kinds_for_domains(&["vision".to_string()]),
            vec!["vision.describe".to_string(), "annotate.asset".to_string()]
        );
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

    /// The two rows that do NOT publish. Everything about them is decided
    /// here, so a coordinator can never publish an answer by accident.
    #[test]
    fn the_vision_kinds_answer_instead_of_publishing() {
        let describe = kind_of("vision.describe").unwrap();
        assert!(describe.is_text());
        assert!(describe.catalog().is_none());
        assert!(describe.advertised());
        assert_eq!(describe.input, InputNeed::Image);

        let annotate = kind_of("annotate.asset").unwrap();
        assert!(annotate.is_annotation());
        assert!(annotate.catalog().is_none());
        // Nobody picks an annotation job from a menu: the server mints it.
        assert!(!annotate.advertised());
        assert_eq!(kind_for_domain("vision").unwrap().kind, "vision.describe");

        // And the kind string IS the wire to the store's queue.
        assert_eq!(annotate.kind, "annotate.asset");
        // Every catalog kind still publishes.
        assert!(kind_of("image.generate").unwrap().catalog().is_some());
    }
}
