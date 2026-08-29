//! Focused release tests for the quantized MiniMax-H3 tier manifests and
//! their fail-closed selection machinery. Hermetic: registry data + pure
//! decision functions only — no GPU, no network, no model files.

use makepad_asset_ai::gpu::GpuInfo;
use makepad_asset_ai::h3_backend::{
    check_canvas_within_tier, check_gpu_requirements, tier_plan_for_spec, H3TierKind,
};
use makepad_asset_ai::registry::Registry;
use makepad_asset_ai::residency::{estimated_peak_mb, DEFAULT_RESERVE_MB};

fn gpu(vram_total_mb: u64, compute_cap: f64) -> GpuInfo {
    GpuInfo {
        name: Some("fixture".to_string()),
        vram_free_mb: Some(vram_total_mb),
        vram_total_mb: Some(vram_total_mb),
        compute_cap: Some(compute_cap),
    }
}

/// The embedded registry's three tier manifests drive the exact fleet
/// placement matrix: q4 on the 4090+, nvfp4 on Blackwell 32GB+, bf16 on the
/// 96GB box — and never the other way around.
#[test]
fn tier_manifests_place_on_the_fleet_exactly() {
    let registry = Registry::embedded().unwrap();
    let q4 = registry.find("minimax-h3-q4-24g").unwrap();
    let nv4 = registry.find("minimax-h3-nvfp4-32g").unwrap();
    let bf16 = registry.find("minimax-h3-bf16-96g").unwrap();
    let legacy = registry.find("minimax-h3").unwrap();
    let fast = registry.find("fasth3-4step").unwrap();
    let fast_q4 = registry.find("fasth3-4step-q4-24g").unwrap();

    // Role-driven tier selection.
    assert_eq!(tier_plan_for_spec(q4).unwrap().kind, H3TierKind::GgufQ4);
    assert_eq!(tier_plan_for_spec(nv4).unwrap().kind, H3TierKind::Nvfp4);
    assert_eq!(tier_plan_for_spec(bf16).unwrap().kind, H3TierKind::Bf16Tree);
    assert_eq!(tier_plan_for_spec(legacy).unwrap().kind, H3TierKind::Bf16Tree);
    // The fast lane = the tree with its DiT swapped: unstaged, no ceiling.
    let fast_plan = tier_plan_for_spec(fast).unwrap();
    assert_eq!(fast_plan.kind, H3TierKind::Bf16Dit);
    assert!(!fast_plan.staged && fast_plan.max_pixel_frames.is_none());
    // The quantized FastH3 tier is a full GgufQ4 manifest (the in-house
    // h3_quant_gguf DiT + the minimax q4 tier's shared TE/VAE set), staged
    // and ceilinged exactly like the minimax q4 tier.
    let fast_q4_plan = tier_plan_for_spec(fast_q4).unwrap();
    assert_eq!(fast_q4_plan.kind, H3TierKind::GgufQ4);
    assert!(fast_q4_plan.staged);
    assert_eq!(
        fast_q4_plan.max_pixel_frames,
        tier_plan_for_spec(q4).unwrap().max_pixel_frames
    );
    assert_eq!(fast_q4.backend, "fast");
    assert!(tier_plan_for_spec(q4).unwrap().staged);
    assert!(tier_plan_for_spec(nv4).unwrap().staged);
    assert!(!tier_plan_for_spec(bf16).unwrap().staged);

    // The fleet placement matrix, driven entirely by registry data.
    let rtx4090 = gpu(24_564, 8.9); // .123
    let rtx5090 = gpu(32_607, 12.0); // .217
    let rtx6000 = gpu(97_887, 12.0); // .169
    let gate = |spec: &makepad_asset_ai::registry::ModelSpec, gpu: &GpuInfo| {
        check_gpu_requirements(&spec.id, spec.min_vram_gb, spec.min_compute_cap, gpu)
    };
    assert!(gate(q4, &rtx4090).is_ok());
    assert!(gate(q4, &rtx5090).is_ok());
    assert!(gate(q4, &rtx6000).is_ok());
    // The quantized FastH3 tier places exactly like the minimax q4 tier.
    assert!(gate(fast_q4, &rtx4090).is_ok());
    assert!(gate(fast_q4, &rtx5090).is_ok());
    assert!(gate(fast_q4, &rtx6000).is_ok());
    assert!(gate(nv4, &rtx4090).is_err(), "nvfp4 must fail closed on sm89");
    assert!(gate(nv4, &rtx5090).is_ok());
    assert!(gate(nv4, &rtx6000).is_ok());
    assert!(gate(bf16, &rtx4090).is_err());
    assert!(gate(bf16, &rtx5090).is_err());
    assert!(gate(bf16, &rtx6000).is_ok());
    // Same VRAM class as the bf16 tree: the 96GB box only.
    assert!(gate(fast, &rtx4090).is_err());
    assert!(gate(fast, &rtx5090).is_err());
    assert!(gate(fast, &rtx6000).is_ok());

    // Service discovery and the fleet scheduler add the safety reserve to
    // `vram_gb`. Pin this second gate against the actual NVML totals: the
    // named quant tiers must remain routable on their real cards, not merely
    // pass the backend's independent min-VRAM/compute-cap checks above.
    let required = |spec: &makepad_asset_ai::registry::ModelSpec| {
        estimated_peak_mb(spec).saturating_add(DEFAULT_RESERVE_MB)
    };
    assert_eq!(required(q4), 22 * 1024);
    assert_eq!(required(fast_q4), 22 * 1024);
    assert_eq!(required(nv4), 30 * 1024);
    assert_eq!(required(bf16), 92 * 1024);
    assert!(required(q4) <= rtx4090.vram_total_mb.unwrap());
    assert!(required(fast_q4) <= rtx4090.vram_total_mb.unwrap());
    assert!(required(nv4) <= rtx5090.vram_total_mb.unwrap());
    assert!(required(bf16) <= rtx6000.vram_total_mb.unwrap());

    // A GPU-less/unknown box refuses every gated tier.
    for spec in [q4, nv4, bf16, fast_q4] {
        assert!(gate(spec, &GpuInfo::default()).is_err(), "{}", spec.id);
    }
    // The legacy alias keeps its ungated behavior.
    assert!(gate(legacy, &GpuInfo::default()).is_ok());
}

/// Measured canvas envelopes: the q4 tier serves the default and 864x480
/// canvases and refuses beyond its pruned-ladder ceiling; nvfp4 serves the
/// full ladder at 124 frames.
#[test]
fn tier_canvas_envelopes_match_the_ladder() {
    let registry = Registry::embedded().unwrap();
    let q4_limit = tier_plan_for_spec(registry.find("minimax-h3-q4-24g").unwrap())
        .unwrap()
        .max_pixel_frames;
    let nv4_limit = tier_plan_for_spec(registry.find("minimax-h3-nvfp4-32g").unwrap())
        .unwrap()
        .max_pixel_frames;
    for (w, h, f) in [(640, 352, 124), (864, 480, 124), (960, 544, 124), (640, 352, 56)] {
        check_canvas_within_tier("q4", q4_limit, w, h, f).unwrap();
    }
    assert!(check_canvas_within_tier("q4", q4_limit, 1344, 768, 124).is_err());
    assert!(check_canvas_within_tier("q4", q4_limit, 960, 544, 243).is_err());
    for (w, h, f) in [(960, 544, 124), (1344, 768, 124)] {
        check_canvas_within_tier("nv4", nv4_limit, w, h, f).unwrap();
    }
    assert!(check_canvas_within_tier("nv4", nv4_limit, 1344, 768, 243).is_err());
}

/// Every quantized-tier file is fully pinned (immutable revision + size +
/// sha256 + role) so the peer cache can distribute and verify byte-exact
/// artifacts; totals match the published repos.
#[test]
fn tier_files_are_fully_pinned_and_sized() {
    let registry = Registry::embedded().unwrap();
    for id in ["minimax-h3-q4-24g", "minimax-h3-nvfp4-32g", "fasth3-4step-q4-24g"] {
        let spec = registry.find(id).unwrap();
        for file in &spec.files {
            assert!(file.role.is_some(), "{id}: {} needs a role", file.cache_as);
            let revision = file.revision.as_deref().unwrap_or_else(|| {
                panic!("{id}: {} needs an immutable revision", file.cache_as)
            });
            assert!(
                revision.len() == 40 && revision.bytes().all(|b| b.is_ascii_hexdigit()),
                "{id}: {} revision must be a commit hash",
                file.cache_as
            );
            assert!(file.size.unwrap_or(0) > 0, "{id}: {}", file.cache_as);
            assert_eq!(
                file.sha256.as_deref().map(str::len),
                Some(64),
                "{id}: {}",
                file.cache_as
            );
        }
    }
    // Download budgets per box: ~35.5 GB for q4, ~34.1 GB for nvfp4 (the
    // 5.8 GB VAE/tokenizer set is shared on disk; both carry the 22.7 MB
    // RIFE interpolate flownet — stale pre-RIFE pins caught here once).
    let total = |id: &str| -> u64 {
        registry
            .find(id)
            .unwrap()
            .files
            .iter()
            .map(|f| f.size.unwrap())
            .sum()
    };
    assert_eq!(total("minimax-h3-q4-24g"), 35_481_501_594);
    assert_eq!(total("minimax-h3-nvfp4-32g"), 34_058_552_150);
    // fasth3-4step-q4-24g = the in-house 11.4GB FastH3 DiT plus the minimax
    // q4 tier's shared TE/VAE/tokenizer set (identical pins, identical cache
    // paths — a box carrying the minimax tier only adds the DiT).
    assert_eq!(total("fasth3-4step-q4-24g"), 35_489_644_339);
    let fast_q4 = registry.find("fasth3-4step-q4-24g").unwrap();
    let dit = fast_q4
        .files
        .iter()
        .find(|f| f.role.as_deref() == Some("dit-gguf"))
        .unwrap();
    // The DiT is a locally-generated artifact: never Hugging Face, peer
    // network only — and digest-pinned so peers can verify it.
    assert!(dit.local);
    assert_eq!(dit.size, Some(11_428_787_200));
    assert_eq!(
        dit.sha256.as_deref(),
        Some("2693c1c6c5218578564306f25fb2bdeeea1f7f6758d126eb37f5644fa47f7b27")
    );
    let q4_spec = registry.find("minimax-h3-q4-24g").unwrap();
    for role in ["te-gguf", "video-vae", "audio-vae", "audio-vae-config", "tokenizer-json"] {
        let ours = fast_q4
            .files
            .iter()
            .find(|f| f.role.as_deref() == Some(role))
            .unwrap();
        let theirs = q4_spec
            .files
            .iter()
            .find(|f| f.role.as_deref() == Some(role))
            .unwrap();
        assert_eq!(ours.cache_as, theirs.cache_as, "{role} must share the cache path");
        assert_eq!(ours.sha256, theirs.sha256, "{role} must share the pinned bytes");
    }
}
