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
//! Most rows publish a REAL catalog asset ([`Product::Catalog`]). Three do
//! not, and they are why the product is a typed decision rather than an
//! assumption: a `vision` box answers a QUESTION about an image, and an
//! answer is not an asset. `vision.describe` records its answer on the job
//! for whoever asked; `annotate.asset` folds it into an existing asset's
//! annotation record; `text.expand` answers with the PROMPT a later stage
//! is going to be handed. `chat` remains absent entirely: nothing in this
//! worker executes it.
//!
//! Two of those are enqueued by software rather than picked by a person
//! ([`GenKind::advertised`]) — which is a different question from whether a
//! worker can claim them. Everything in this table is claimable.

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
    /// Does a PICKER offer this kind?
    ///
    /// Data, not a derivation, because two rows are enqueued by software
    /// rather than chosen by a person and they have nothing else in common:
    /// `annotate.asset` is minted by the store for an asset that already
    /// exists, and `text.expand` is named by a pipeline as its first stage.
    /// Both are fully claimable; neither has a prompt a human writes at a
    /// menu, and a menu entry that produces no visible product is a lie.
    pub advertised: bool,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
    },
    GenKind {
        // The prompt expander, as a REAL job.
        //
        // It has always existed as the coordinator's private pre-step in
        // front of a generation (`expand: true`), where it is invisible
        // until it has already run. A pipeline needs it as a STAGE: named
        // at spawn, cancellable, inspectable, and — the point — with a
        // recorded result a dependent stage can splice its own body from
        // (`{"$from":"job_…","field":"prompt"}`).
        //
        // Its result is TEXT on the job, never a catalog row: an expansion
        // is a sentence, and a sentence is not an asset. `prompt` is always
        // flattened onto that result; a music answer also flattens
        // `lyrics`/`seconds`, which is what a music stage splices.
        kind: "text.expand",
        domain: "text",
        product: Product::Text,
        input: InputNeed::None,
        action: "expand a prompt",
        // Never in a picker: nobody asks for a prompt about a prompt, and a
        // menu entry whose product is invisible reads as a broken generate.
        // A pipeline names this kind directly.
        advertised: false,
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
        advertised: true,
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
        advertised: false,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        advertised: true,
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
        .find(|k| k.domain == domain && k.advertised)
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
            // A domain may carry several kinds, but AT MOST ONE of them is
            // advertised — two profiles for one capability would put the
            // same GPU behind two picker entries that mean the same thing.
            // Zero is legal: `text` is claimable and never offered.
            assert!(
                GEN_KINDS
                    .iter()
                    .filter(|o| o.domain == row.domain && o.advertised)
                    .count()
                    <= 1,
                "{}: duplicate advertised domain",
                row.domain
            );
            // An answer folded into somebody else's asset can never be a
            // menu entry: there is no prompt for a person to write.
            assert!(!(row.is_annotation() && row.advertised), "{}", row.kind);
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
        // A box that answers text claims the expander: `text.expand` is a
        // real queued job now, not the coordinator's private pre-step.
        assert_eq!(
            kinds_for_domains(&domains),
            vec![
                "image.generate".to_string(),
                "text.expand".to_string(),
                "video.generate".to_string()
            ]
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
        assert_eq!(kind_of("text.expand").unwrap().domain, "text");
        // Claimable, never offered: `kind_for_domain` answers what a PICKER
        // may show, and the expander is named by a pipeline instead.
        assert!(kind_for_domain("text").is_none());
        assert!(kind_for_domain("chat").is_none());
        assert_eq!(InputNeed::Mesh.content_type(), "model/gltf-binary");
        assert_eq!(InputNeed::None.content_type(), "image/png");
    }

    /// The three rows that do NOT publish. Everything about them is decided
    /// here, so a coordinator can never publish an answer by accident.
    #[test]
    fn the_vision_kinds_answer_instead_of_publishing() {
        let describe = kind_of("vision.describe").unwrap();
        assert!(describe.is_text());
        assert!(describe.catalog().is_none());
        assert!(describe.advertised);
        assert_eq!(describe.input, InputNeed::Image);

        let annotate = kind_of("annotate.asset").unwrap();
        assert!(annotate.is_annotation());
        assert!(annotate.catalog().is_none());
        // Nobody picks an annotation job from a menu: the server mints it.
        assert!(!annotate.advertised);
        assert_eq!(kind_for_domain("vision").unwrap().kind, "vision.describe");

        // And the kind string IS the wire to the store's queue.
        assert_eq!(annotate.kind, "annotate.asset");
        // Every catalog kind still publishes.
        assert!(kind_of("image.generate").unwrap().catalog().is_some());
    }

    /// The expander is a claimable job kind whose answer is a PROMPT. It is
    /// the first stage of every pipeline that has one, so a client can
    /// enqueue it and a text box can claim it — while no picker offers it.
    #[test]
    fn the_expander_is_claimable_but_never_offered() {
        let expand = kind_of("text.expand").expect("text.expand is wired");
        assert_eq!(expand.domain, "text");
        assert!(expand.is_text(), "an expansion is text on the job");
        assert!(expand.catalog().is_none(), "an expansion is not an asset");
        assert!(!expand.is_annotation());
        assert!(!expand.advertised, "a pipeline names it; a menu never does");
        // The box that runs text models claims it — the whole point of the
        // promotion. (.217 is the fleet's text/chat box; its role allows
        // `text`, so the claim filter and the router agree.)
        assert!(kinds_for_domains(&["text".to_string()])
            .contains(&"text.expand".to_string()));
        assert!(makepad_asset_ai::fleet::role_allows(
            "http://10.0.0.217:8123",
            "text"
        ));
    }
}
