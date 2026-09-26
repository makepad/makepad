//! Render-equivalence check for the repack policy.
//!
//! The repacker's own verifier proves byte-level facts (kept sections are
//! unchanged, dropped ones are exactly the policy's). This proves the claim
//! the policy rests on: the renderer builds the SAME GPU buffers from the
//! repacked tile as it did from the original. For every render bucket and
//! mode it builds
//! - A: the original tile, parsed as before the detail contract existed
//!   (renderer filter off),
//! - B: the repacked tile, parsed with the contract (production today),
//! and requires A == B over every emitted stream, label and instance, plus
//! the same count of baked field-101 hits (so a drop that silently
//! invalidated a baked cascade shows up even though pixels would match).
//!
//! It needs the renderer, so it lives under the `faces` feature. The
//! renderer filter is a process-wide switch: run checks on one thread.

use makepad_widgets::map::geometry::TileKey;
use makepad_widgets::map::style::probe_compiled_theme;
use makepad_widgets::map::tile::{
    baked_faces_hits, build_local_tile_from_archive_bytes, set_detail_contract_filter,
    TileBuffers,
};
use std::sync::Arc;

/// The modes a check covers: (render bucket, 3D buildings, baked fringe).
pub fn default_render_cases(tile_zoom: u32) -> Vec<(u32, bool, bool)> {
    let mut cases = Vec::new();
    let buckets: Vec<u32> = if tile_zoom >= 14 {
        (14..=18).collect()
    } else {
        vec![tile_zoom]
    };
    for bucket in buckets {
        cases.push((bucket, false, true));
        cases.push((bucket, true, false));
    }
    cases
}

#[derive(Clone, Debug, Default)]
pub struct RenderCheckStats {
    pub tiles: usize,
    pub builds: usize,
    pub baked_hits_before: u64,
    pub baked_hits_after: u64,
    pub mismatches: Vec<String>,
}

fn build(
    key: TileKey,
    decoded: &Arc<[u8]>,
    bucket: u32,
    buildings_3d: bool,
    fringe: bool,
    contract: bool,
) -> Result<(Option<TileBuffers>, u64), String> {
    let theme = probe_compiled_theme();
    set_detail_contract_filter(contract);
    let hits = baked_faces_hits();
    // A combined archive's detail pass reads the base bytes themselves from
    // bucket 14 (view.rs: reuse_base_as_detail).
    let result = build_local_tile_from_archive_bytes(
        key,
        Some(decoded.clone()),
        (bucket >= 14).then(|| decoded.clone()),
        None,
        None,
        Vec::new(),
        &[],
        &theme,
        bucket,
        buildings_3d,
        fringe,
        true,
    );
    let hits = baked_faces_hits() - hits;
    set_detail_contract_filter(true);
    let mut buffers = result?.map(|tile| tile.buffers);
    if let Some(buffers) = buffers.as_mut() {
        // Timing text and the parsed-feature tally are not render output;
        // the tally legitimately drops with the features.
        buffers.stage_summary.clear();
        buffers.feature_count = 0;
    }
    Ok((buffers, hits))
}

/// Check one tile given as decoded (uncompressed) payloads before and
/// after the rewrite.
pub fn check_tile(
    (z, x, y): (u32, u32, u32),
    before: &[u8],
    after: &[u8],
    stats: &mut RenderCheckStats,
) -> Result<(), String> {
    let key = TileKey { z, x: x as i32, y: y as i32 };
    let before: Arc<[u8]> = Arc::from(before);
    let after: Arc<[u8]> = Arc::from(after);
    stats.tiles += 1;
    for (bucket, buildings_3d, fringe) in default_render_cases(key.z) {
        // Warm-up: first builds register icons/props in process-wide
        // tables, which shifts ids baked into later builds.
        build(key, &before, bucket, buildings_3d, fringe, false)?;
        build(key, &after, bucket, buildings_3d, fringe, true)?;
        let (a, hits_a) = build(key, &before, bucket, buildings_3d, fringe, false)?;
        let (b, hits_b) = build(key, &after, bucket, buildings_3d, fringe, true)?;
        stats.builds += 2;
        stats.baked_hits_before += hits_a;
        stats.baked_hits_after += hits_b;
        let case = format!(
            "z{}/{}/{} bucket {bucket} {} fringe={fringe}",
            key.z,
            key.x,
            key.y,
            if buildings_3d { "3d" } else { "2d" }
        );
        if digest(&a) != digest(&b) {
            let (a2, _) = build(key, &before, bucket, buildings_3d, fringe, false)?;
            let (c, _) = build(key, &before, bucket, buildings_3d, fringe, true)?;
            stats.mismatches.push(format!(
                "{case}: buffers differ{}; original rebuilt {}; original+filter vs repacked {}",
                describe(&a, &b),
                if digest(&a) == digest(&a2) { "same" } else { "DIFFERS (nondeterministic build)" },
                if digest(&c) == digest(&b) { "same" } else { "differ" },
            ));
        } else if hits_a != hits_b {
            stats
                .mismatches
                .push(format!("{case}: baked faces hit {hits_a} before, {hits_b} after"));
        }
    }
    Ok(())
}

/// FNV over the Debug rendering. Vertex streams carry NaN markers, so
/// `PartialEq` calls identical buffers different; Debug prints every float
/// by value (NaN as "NaN"), which makes this a bitwise-strength compare.
fn digest(value: &impl std::fmt::Debug) -> u64 {
    struct Fnv(u64);
    impl std::fmt::Write for Fnv {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            for byte in text.bytes() {
                self.0 = (self.0 ^ byte as u64).wrapping_mul(0x100_0000_01b3);
            }
            Ok(())
        }
    }
    let mut hasher = Fnv(0xcbf2_9ce4_8422_2325);
    let _ = std::fmt::write(&mut hasher, format_args!("{value:?}"));
    hasher.0
}

fn len_pair<T: std::fmt::Debug>(a: &[T], b: &[T]) -> String {
    let first = a.iter().zip(b.iter()).position(|(x, y)| digest(x) != digest(y));
    format!(" len {}/{} first diff at {:?}", a.len(), b.len(), first)
}

fn differing_fields(a: &TileBuffers, b: &TileBuffers) -> Vec<String> {
    let mut out = Vec::new();
    if digest(&a.pin_hits) != digest(&b.pin_hits) {
        out.push(format!("pin_hits{}", len_pair(&a.pin_hits, &b.pin_hits)));
    }
    if digest(&a.fill) != digest(&b.fill) {
        out.push("fill".to_string());
    }
    if digest(&a.fill_misc_indices) != digest(&b.fill_misc_indices) {
        out.push(format!("fill_misc_indices{}", len_pair(&a.fill_misc_indices, &b.fill_misc_indices)));
    }
    if digest(&a.fill_misc_vertices) != digest(&b.fill_misc_vertices) {
        out.push(format!("fill_misc_vertices{}", len_pair(&a.fill_misc_vertices, &b.fill_misc_vertices)));
    }
    if digest(&a.face) != digest(&b.face) {
        out.push("face".to_string());
    }
    if digest(&a.casing) != digest(&b.casing) {
        out.push("casing".to_string());
    }
    if digest(&a.stroke) != digest(&b.stroke) {
        out.push("stroke".to_string());
    }
    if digest(&a.icon_indices) != digest(&b.icon_indices) {
        out.push(format!("icon_indices{}", len_pair(&a.icon_indices, &b.icon_indices)));
    }
    if digest(&a.icon_vertices) != digest(&b.icon_vertices) {
        out.push(format!("icon_vertices{}", len_pair(&a.icon_vertices, &b.icon_vertices)));
    }
    if digest(&a.icon_high_indices) != digest(&b.icon_high_indices) {
        out.push(format!("icon_high_indices{}", len_pair(&a.icon_high_indices, &b.icon_high_indices)));
    }
    if digest(&a.icon_high_vertices) != digest(&b.icon_high_vertices) {
        out.push(format!("icon_high_vertices{}", len_pair(&a.icon_high_vertices, &b.icon_high_vertices)));
    }
    if digest(&a.icon_instances) != digest(&b.icon_instances) {
        out.push(format!("icon_instances{}", len_pair(&a.icon_instances, &b.icon_instances)));
    }
    if digest(&a.icon_high_instances) != digest(&b.icon_high_instances) {
        out.push(format!("icon_high_instances{}", len_pair(&a.icon_high_instances, &b.icon_high_instances)));
    }
    if digest(&a.shadow_disc_instances) != digest(&b.shadow_disc_instances) {
        out.push(format!("shadow_disc_instances{}", len_pair(&a.shadow_disc_instances, &b.shadow_disc_instances)));
    }
    if digest(&a.fringe) != digest(&b.fringe) {
        out.push("fringe".to_string());
    }
    if digest(&a.fill_3d) != digest(&b.fill_3d) {
        out.push("fill_3d".to_string());
    }
    if digest(&a.fill_3d_misc_indices) != digest(&b.fill_3d_misc_indices) {
        out.push(format!("fill_3d_misc_indices{}", len_pair(&a.fill_3d_misc_indices, &b.fill_3d_misc_indices)));
    }
    if digest(&a.fill_3d_misc_vertices) != digest(&b.fill_3d_misc_vertices) {
        out.push(format!("fill_3d_misc_vertices{}", len_pair(&a.fill_3d_misc_vertices, &b.fill_3d_misc_vertices)));
    }
    if digest(&a.wall_indices) != digest(&b.wall_indices) {
        out.push(format!("wall_indices{}", len_pair(&a.wall_indices, &b.wall_indices)));
    }
    if digest(&a.wall_vertices) != digest(&b.wall_vertices) {
        out.push(format!("wall_vertices{}", len_pair(&a.wall_vertices, &b.wall_vertices)));
    }
    if digest(&a.wall_instances) != digest(&b.wall_instances) {
        out.push(format!("wall_instances{}", len_pair(&a.wall_instances, &b.wall_instances)));
    }
    if digest(&a.tree_indices) != digest(&b.tree_indices) {
        out.push(format!("tree_indices{}", len_pair(&a.tree_indices, &b.tree_indices)));
    }
    if digest(&a.tree_vertices) != digest(&b.tree_vertices) {
        out.push(format!("tree_vertices{}", len_pair(&a.tree_vertices, &b.tree_vertices)));
    }
    if digest(&a.tree_cross_indices) != digest(&b.tree_cross_indices) {
        out.push(format!("tree_cross_indices{}", len_pair(&a.tree_cross_indices, &b.tree_cross_indices)));
    }
    if digest(&a.tree_cross_vertices) != digest(&b.tree_cross_vertices) {
        out.push(format!("tree_cross_vertices{}", len_pair(&a.tree_cross_vertices, &b.tree_cross_vertices)));
    }
    if digest(&a.tree_template_indices) != digest(&b.tree_template_indices) {
        out.push(format!("tree_template_indices{}", len_pair(&a.tree_template_indices, &b.tree_template_indices)));
    }
    if digest(&a.tree_template_vertices) != digest(&b.tree_template_vertices) {
        out.push(format!("tree_template_vertices{}", len_pair(&a.tree_template_vertices, &b.tree_template_vertices)));
    }
    if digest(&a.tree_cross_template_indices) != digest(&b.tree_cross_template_indices) {
        out.push(format!("tree_cross_template_indices{}", len_pair(&a.tree_cross_template_indices, &b.tree_cross_template_indices)));
    }
    if digest(&a.tree_cross_template_vertices) != digest(&b.tree_cross_template_vertices) {
        out.push(format!("tree_cross_template_vertices{}", len_pair(&a.tree_cross_template_vertices, &b.tree_cross_template_vertices)));
    }
    if digest(&a.tree_instances) != digest(&b.tree_instances) {
        out.push(format!("tree_instances{}", len_pair(&a.tree_instances, &b.tree_instances)));
    }
    if digest(&a.stalk_template_indices) != digest(&b.stalk_template_indices) {
        out.push(format!("stalk_template_indices{}", len_pair(&a.stalk_template_indices, &b.stalk_template_indices)));
    }
    if digest(&a.stalk_template_vertices) != digest(&b.stalk_template_vertices) {
        out.push(format!("stalk_template_vertices{}", len_pair(&a.stalk_template_vertices, &b.stalk_template_vertices)));
    }
    if digest(&a.stalk_instances) != digest(&b.stalk_instances) {
        out.push(format!("stalk_instances{}", len_pair(&a.stalk_instances, &b.stalk_instances)));
    }
    if digest(&a.stoplight_template_indices) != digest(&b.stoplight_template_indices) {
        out.push(format!("stoplight_template_indices{}", len_pair(&a.stoplight_template_indices, &b.stoplight_template_indices)));
    }
    if digest(&a.stoplight_template_vertices) != digest(&b.stoplight_template_vertices) {
        out.push(format!("stoplight_template_vertices{}", len_pair(&a.stoplight_template_vertices, &b.stoplight_template_vertices)));
    }
    if digest(&a.stoplight_instances) != digest(&b.stoplight_instances) {
        out.push(format!("stoplight_instances{}", len_pair(&a.stoplight_instances, &b.stoplight_instances)));
    }
    if digest(&a.road_icon_indices) != digest(&b.road_icon_indices) {
        out.push(format!("road_icon_indices{}", len_pair(&a.road_icon_indices, &b.road_icon_indices)));
    }
    if digest(&a.road_icon_vertices) != digest(&b.road_icon_vertices) {
        out.push(format!("road_icon_vertices{}", len_pair(&a.road_icon_vertices, &b.road_icon_vertices)));
    }
    if digest(&a.mode_overlay_only) != digest(&b.mode_overlay_only) {
        out.push("mode_overlay_only".to_string());
    }
    if digest(&a.labels) != digest(&b.labels) {
        out.push(format!("labels{}", len_pair(&a.labels, &b.labels)));
    }
    if digest(&a.render_zoom) != digest(&b.render_zoom) {
        out.push("render_zoom".to_string());
    }
    if digest(&a.memory_lod) != digest(&b.memory_lod) {
        out.push("memory_lod".to_string());
    }
    out
}

fn describe(a: &Option<TileBuffers>, b: &Option<TileBuffers>) -> String {
    match (a, b) {
        (Some(a), Some(b)) => {
            let (sa, sb) = (a.stream_bytes(), b.stream_bytes());
            let streams: Vec<String> = sa
                .iter()
                .zip(sb.iter())
                .enumerate()
                .filter(|(_, (x, y))| x != y)
                .map(|(index, (x, y))| format!("stream{index} {x}->{y}"))
                .collect();
            format!(
                " (fields {:?}; {}; labels {}->{})",
                differing_fields(a, b),
                if streams.is_empty() {
                    "same stream sizes".to_string()
                } else {
                    streams.join(", ")
                },
                a.labels.len(),
                b.labels.len()
            )
        }
        (a, b) => format!(" (tile present before={} after={})", a.is_some(), b.is_some()),
    }
}
