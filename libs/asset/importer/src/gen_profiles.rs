//! Building the advertised generation profiles from what the fleet can
//! ACTUALLY execute right now.
//!
//! The Asset Server's config advertisement is a deployment intention; this
//! module turns a live fleet snapshot into the truth. A profile appears
//! only when some box advertises the model, marks it available, fits it in
//! that GPU, and already holds its weights — a tier whose 74 GB of weights
//! are not on any box is a job that would sit pending forever, which is the
//! precise failure this whole lane exists to end.
//!
//! The worker covers EVERY kind in [`crate::gen_kinds::GEN_KINDS`], so it
//! announces all of those domains: covering a domain and advertising
//! nothing in it is how a stale config profile gets withdrawn — and it is
//! also the honest report for a domain whose kind is never offered
//! (`text`, whose expander a pipeline names and no picker shows).

use crate::gen_kinds::{GenKind, GEN_KINDS};
use makepad_ai_hub::fleet::BoxSnapshot;
use makepad_ai_hub::protocol::{ModelInfoJson, MODEL_STATE_LOADED, MODEL_STATE_READY};
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::JobProfileDto;

/// One advertised tier of a domain. Presets exist so a picker can offer
/// "short/long/wide" without the client knowing model internals; the
/// defaults ride into the job body and the worker forwards them verbatim.
struct Preset {
    /// Id suffix; empty = the domain's single profile.
    suffix: &'static str,
    /// Label suffix describing the tier; empty = use the kind's action.
    label: &'static str,
    /// Extra job-body defaults merged after `model`.
    defaults: &'static [(&'static str, i64)],
}

const ONE: &[Preset] = &[Preset { suffix: "", label: "", defaults: &[] }];

/// The `fast` video backend (FastH3, the DMD2-distilled MiniMax H3): the
/// same three canvases, but `steps` counts DiT forwards and 4 is the
/// trained schedule — an H3 tier's 30/50 would be the wrong unit here.
const VIDEO_FAST: &[Preset] = &[
    Preset {
        suffix: "standard",
        label: "640×352 · ~2.7s · 4-step",
        defaults: &[("width", 640), ("height", 352), ("frames", 65), ("steps", 4)],
    },
    Preset {
        suffix: "long",
        label: "640×352 · ~5.4s · 4-step",
        defaults: &[("width", 640), ("height", 352), ("frames", 129), ("steps", 4)],
    },
    Preset {
        suffix: "wide",
        label: "960×544 · ~2.7s · 4-step",
        defaults: &[("width", 960), ("height", 544), ("frames", 65), ("steps", 4)],
    },
];

/// Video is the one domain whose cost varies enough that a picker needs
/// tiers. These mirror the server's stock advertisement so the VJ's queue
/// estimates keep meaning the same thing.
const VIDEO: &[Preset] = &[
    Preset {
        suffix: "standard",
        label: "640×352 · ~2.7s",
        defaults: &[("width", 640), ("height", 352), ("frames", 65), ("steps", 30)],
    },
    Preset {
        suffix: "long",
        label: "640×352 · ~5.4s",
        defaults: &[("width", 640), ("height", 352), ("frames", 129), ("steps", 50)],
    },
    Preset {
        suffix: "wide",
        label: "960×544 · ~2.7s",
        defaults: &[("width", 960), ("height", 544), ("frames", 65), ("steps", 30)],
    },
];

fn presets_for(domain: &str, backend: &str) -> &'static [Preset] {
    match (domain, backend) {
        ("video", "fast") => VIDEO_FAST,
        ("video", _) => VIDEO,
        _ => ONE,
    }
}

/// Profile ids are restricted to `[a-z0-9-_]`; model ids are not (`.` shows
/// up in `ace-step-1.5-xl` and in `qwen3.8-27b-vision`). Fold anything else
/// to `-` and collapse runs so two models can never collide on a shared
/// prefix. Also the label-safe model identity the annotation pass publishes
/// as its `vlm-m-<tag>` tag: same charset rule, same answer.
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// Weights are on the box (or already resident). `downloading`/`absent`
/// models are deliberately NOT advertised: they are capabilities the fleet
/// could acquire, not work it can do now.
fn is_executable_now(model: &ModelInfoJson) -> bool {
    model.available && (model.state == MODEL_STATE_LOADED || model.state == MODEL_STATE_READY)
}

/// Reference/oracle backends are explicit-pin only — never advertised, for
/// the same reason domain affinity skips them: a Python oracle must not
/// look like the canonical runtime.
fn is_advertisable(model: &ModelInfoJson) -> bool {
    !model.backend.ends_with("-oracle") && model.backend != "testpattern"
}

/// Does any live box actually fit this model? Hardware compatibility is a
/// permanent fact of the pairing, unlike transient VRAM pressure, so it is
/// the only memory gate applied to ADVERTISEMENT (a busy box still serves
/// the profile — the job just waits).
fn hardware_fits(snapshots: &[BoxSnapshot], model_id: &str) -> bool {
    snapshots.iter().any(|snapshot| {
        makepad_ai_hub::fleet::model_admission(snapshot, model_id)
            .is_some_and(|a| a.is_hardware_compatible())
    })
}

/// Every domain this worker is authoritative for (the whole wired table),
/// each named once — a domain that carries two kinds (`vision`: the
/// client's question and the catalog's annotation pass) is still one
/// capability the worker covers.
pub fn covered_domains() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for row in GEN_KINDS {
        if !out.iter().any(|d| d == row.domain) {
            out.push(row.domain.to_string());
        }
    }
    out
}

/// Build the advertisement for one fleet snapshot, in table order. `ns` is
/// the namespace jobs of these profiles are enqueued under.
pub fn build_profiles(snapshots: &[BoxSnapshot], ns: &str) -> Vec<JobProfileDto> {
    let mut out: Vec<JobProfileDto> = Vec::new();
    for row in GEN_KINDS {
        // An annotation job is minted by the server for an asset that
        // already exists: there is no prompt for a human to write and
        // nothing for a picker to offer.
        if !row.advertised {
            continue;
        }
        for (model, backend) in executable_models(snapshots, row) {
            for preset in presets_for(row.domain, &backend) {
                if let Some(profile) = profile_of(row, &model, preset, ns) {
                    if !out.iter().any(|p| p.id == profile.id) {
                        out.push(profile);
                    }
                }
            }
        }
    }
    out
}

/// Model ids of `row`'s domain that some live box can execute now, in
/// stable (box order, service order) order so the advertisement does not
/// churn between polls.
/// `(model id, backend)` of every model the fleet can run for `row`, the
/// domain's PREFERRED backend first (the fleet scheduler routes unpinned
/// jobs there, so its profiles lead the picker too), then fleet order.
fn executable_models(snapshots: &[BoxSnapshot], row: &GenKind) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for snapshot in snapshots {
        // A box outside its role does not serve this domain, so its copy of
        // the weights is not a capability the fleet can offer.
        if !snapshot.is_up()
            || !makepad_ai_hub::fleet::role_allows(&snapshot.base_url, row.domain)
        {
            continue;
        }
        for model in &snapshot.models {
            if model.domain != row.domain
                || !is_advertisable(model)
                || !is_executable_now(model)
                || out.iter().any(|(id, _)| id == &model.id)
                || !hardware_fits(snapshots, &model.id)
            {
                continue;
            }
            out.push((model.id.clone(), model.backend.clone()));
        }
    }
    // Stable: preferred backends float to the front, everything else keeps
    // its discovery order.
    let (preferred, rest): (Vec<_>, Vec<_>) = out
        .into_iter()
        .partition(|(_, backend)| makepad_ai_hub::fleet::is_preferred_domain_backend(row.domain, backend));
    preferred.into_iter().chain(rest).collect()
}

fn profile_of(
    row: &GenKind,
    model: &str,
    preset: &Preset,
    ns: &str,
) -> Option<JobProfileDto> {
    let model_slug = slug(model);
    if model_slug.is_empty() {
        return None;
    }
    let mut id = format!("{}-{}", row.domain, model_slug);
    if !preset.suffix.is_empty() {
        id.push('-');
        id.push_str(preset.suffix);
    }
    if id.len() > 64 {
        id.truncate(64);
        while id.ends_with('-') {
            id.pop();
        }
    }
    let label = if preset.label.is_empty() {
        format!("{model} · {}", row.action)
    } else {
        format!("{model} · {}", preset.label)
    };
    let mut defaults = vec![("model", s(model.to_string()))];
    for (key, value) in preset.defaults {
        defaults.push((*key, Value::Int(*value)));
    }
    Some(JobProfileDto {
        id,
        domain: row.domain.to_string(),
        label,
        kind: row.kind.to_string(),
        namespace: ns.to_string(),
        defaults: obj(defaults),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_hub::protocol::{HealthJson, MODEL_STATE_ABSENT};

    fn model(id: &str, domain: &str, state: &str, vram_gb: f64) -> ModelInfoJson {
        ModelInfoJson {
            id: id.to_string(),
            domain: domain.to_string(),
            backend: domain.to_string(),
            available: true,
            gated: false,
            vram_gb: Some(vram_gb),
            note: None,
            state: state.to_string(),
            progress_done: None,
            progress_total: None,
            downloading_file: None,
            error: None,
            revision: None,
            unavailable_reason: None,
            license_name: None,
            license_url: None,
            license_summary: None,
            license_restriction: None,
            license_sha256: None,
        }
    }

    fn snapshot(url: &str, total_mb: u64, models: Vec<ModelInfoJson>) -> BoxSnapshot {
        BoxSnapshot {
            base_url: url.to_string(),
            health: Some(HealthJson {
                service: "makepad-asset-ai".to_string(),
                version: "test".to_string(),
                gpu: None,
                vram_free_mb: Some(total_mb),
                vram_total_mb: Some(total_mb),
                models_loaded: Vec::new(),
                jobs_pending: Some(0),
                node_id: None,
                node_key: None,
                started_ms: None,
                capabilities: None,
                vram_reserve_mb: Some(2048),
                queue_limit: Some(8),
                fleet: None,
                lanes: None,
                realtime: None,
            }),
            models,
        }
    }

    #[test]
    fn only_executable_models_are_advertised() {
        let snapshots = vec![snapshot(
            "http://big",
            96 * 1024,
            vec![
                model("flux1-schnell", "image", MODEL_STATE_READY, 21.0),
                model("flux1-dev", "image", MODEL_STATE_ABSENT, 21.0),
                model("minimax-h3", "video", MODEL_STATE_ABSENT, 74.0),
                model("minimax-music3", "music", MODEL_STATE_LOADED, 29.0),
            ],
        )];
        let profiles = build_profiles(&snapshots, "gen");
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        // Ready + loaded appear; the two `absent` tiers do not — advertising
        // a 74 GB download as a capability is what starved the queue.
        assert_eq!(ids, vec!["image-flux1-schnell", "music-minimax-music3"]);
        let image = &profiles[0];
        assert_eq!(image.kind, "image.generate");
        assert_eq!(image.namespace, "gen");
        assert_eq!(
            image.defaults.get("model").and_then(Value::as_str),
            Some("flux1-schnell")
        );
    }

    #[test]
    fn a_video_model_advertises_its_three_stock_tiers() {
        let snapshots = vec![snapshot(
            "http://big",
            96 * 1024,
            vec![model("minimax-h3", "video", MODEL_STATE_READY, 74.0)],
        )];
        let profiles = build_profiles(&snapshots, "gen");
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "video-minimax-h3-standard",
                "video-minimax-h3-long",
                "video-minimax-h3-wide"
            ]
        );
        assert_eq!(
            profiles[1].defaults.get("frames").and_then(Value::as_u64),
            Some(129)
        );
        assert!(profiles.iter().all(|p| p.kind == "video.generate"));
    }

    /// The fast lane leads the video picker with its own step unit: the
    /// distilled model's presets carry `steps: 4` (DiT forwards), the H3
    /// tier keeps 30/50, and the fast profiles come first even when the
    /// fleet lists the H3 tier before it.
    #[test]
    fn the_fast_video_lane_leads_the_picker_with_four_step_presets() {
        let mut fast = model("fasth3-4step", "video", MODEL_STATE_READY, 74.0);
        fast.backend = "fast".to_string();
        let mut h3 = model("minimax-h3-bf16-96g", "video", MODEL_STATE_LOADED, 74.0);
        h3.backend = "h3".to_string();
        let snapshots = vec![snapshot("http://big", 96 * 1024, vec![h3, fast])];
        let profiles = build_profiles(&snapshots, "gen");
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "video-fasth3-4step-standard",
                "video-fasth3-4step-long",
                "video-fasth3-4step-wide",
                "video-minimax-h3-bf16-96g-standard",
                "video-minimax-h3-bf16-96g-long",
                "video-minimax-h3-bf16-96g-wide",
            ]
        );
        for profile in &profiles[..3] {
            assert_eq!(profile.defaults.get("steps").and_then(Value::as_u64), Some(4));
            assert_eq!(
                profile.defaults.get("model").and_then(Value::as_str),
                Some("fasth3-4step")
            );
        }
        assert_eq!(profiles[3].defaults.get("steps").and_then(Value::as_u64), Some(30));
        assert_eq!(profiles[4].defaults.get("steps").and_then(Value::as_u64), Some(50));
        assert!(profiles[0].label.ends_with("4-step"));
    }

    #[test]
    fn an_undersized_gpu_never_advertises_a_model_it_cannot_hold() {
        // 24 GB card, 74 GB model marked ready (stale registry row): the
        // hardware gate is permanent, so it must not be advertised.
        let snapshots = vec![snapshot(
            "http://small",
            24 * 1024,
            vec![
                model("minimax-h3", "video", MODEL_STATE_READY, 74.0),
                model("flux1-schnell", "image", MODEL_STATE_READY, 21.0),
            ],
        )];
        let ids: Vec<String> = build_profiles(&snapshots, "gen")
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["image-flux1-schnell"]);
    }

    #[test]
    fn oracles_test_patterns_and_down_boxes_are_never_advertised() {
        let mut oracle = model("hy-motion-oracle", "motion", MODEL_STATE_LOADED, 24.0);
        oracle.backend = "motion-oracle".to_string();
        let mut tp = model("testpattern", "image", MODEL_STATE_LOADED, 1.0);
        tp.backend = "testpattern".to_string();
        let mut down = snapshot(
            "http://down",
            96 * 1024,
            vec![model("flux2-dev", "image", MODEL_STATE_READY, 29.0)],
        );
        down.health = None;
        let snapshots = vec![
            down,
            snapshot("http://up", 96 * 1024, vec![oracle, tp]),
        ];
        assert!(build_profiles(&snapshots, "gen").is_empty());
    }

    #[test]
    fn two_boxes_serving_one_model_advertise_it_once() {
        let snapshots = vec![
            snapshot(
                "http://a",
                96 * 1024,
                vec![model("flux1-schnell", "image", MODEL_STATE_READY, 21.0)],
            ),
            snapshot(
                "http://b",
                96 * 1024,
                vec![model("flux1-schnell", "image", MODEL_STATE_LOADED, 21.0)],
            ),
        ];
        assert_eq!(build_profiles(&snapshots, "gen").len(), 1);
    }

    #[test]
    fn ids_stay_inside_the_servers_charset() {
        let snapshots = vec![snapshot(
            "http://big",
            96 * 1024,
            vec![model("ACE-Step-1.5-XL", "music", MODEL_STATE_READY, 12.0)],
        )];
        let profiles = build_profiles(&snapshots, "gen");
        assert_eq!(profiles[0].id, "music-ace-step-1-5-xl");
        assert!(profiles[0].id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_'
        }));
        // The label keeps the real model id — the slug is an id, not a name.
        assert!(profiles[0].label.starts_with("ACE-Step-1.5-XL"));
    }

    /// A box outside its role advertises nothing for the domains it may not
    /// serve. The dedicated chat node honestly reports `video` in its
    /// capabilities; that must not turn into a picker entry, and its lone
    /// copy of a model must not make that model look fleet-available.
    /// (`.165` is the ratified dedicated chat box — see the hub's
    /// `DEFAULT_FLEET_ROLES`.)
    #[test]
    fn a_role_excluded_box_advertises_nothing_for_that_domain() {
        if std::env::var(makepad_ai_hub::fleet::FLEET_ROLES_ENV).is_ok() {
            return;
        }
        let snapshots = vec![
            snapshot(
                "http://10.0.0.165:8123",
                32 * 1024,
                vec![
                    model("qwen3.8-27b", "chat", MODEL_STATE_LOADED, 24.0),
                    model("minimax-h3-q4-24g", "video", MODEL_STATE_READY, 20.0),
                ],
            ),
            snapshot(
                "http://10.0.0.123:8123",
                24 * 1024,
                vec![model("flux1-schnell", "image", MODEL_STATE_READY, 21.0)],
            ),
        ];
        let ids: Vec<String> = build_profiles(&snapshots, "gen")
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["image-flux1-schnell".to_string()]);
    }

    #[test]
    fn the_worker_covers_every_wired_domain() {
        let domains = covered_domains();
        let mut wired: Vec<&str> = GEN_KINDS.iter().map(|k| k.domain).collect();
        wired.sort_unstable();
        wired.dedup();
        assert_eq!(domains.len(), wired.len());
        // Named once, even though `vision` carries two kinds.
        assert_eq!(domains.iter().filter(|d| *d == "vision").count(), 1);
        assert!(domains.iter().any(|d| d == "video"));
        // COVERED IS NOT OFFERED. The worker executes `text.expand`, so it
        // says it covers `text`; a picker is still never given a "expand a
        // prompt" entry, because its product is not a thing a person browses
        // for.
        assert!(domains.iter().any(|d| d == "text"));
        let text_box = vec![snapshot(
            "http://big",
            96 * 1024,
            vec![model("qwen3.8-27b", "text", MODEL_STATE_READY, 24.0)],
        )];
        assert!(build_profiles(&text_box, "gen").is_empty());
        // Covering `video` while advertising nothing is what withdraws the
        // deployment's stock H3 profiles when no box holds those weights.
        assert!(build_profiles(&[], "gen").is_empty());
    }
}
