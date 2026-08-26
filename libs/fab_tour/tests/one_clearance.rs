//! The one-clearance law, asserted.
//!
//! The Doom walker's planner and its body used two nearly-identical wall
//! tests. The sliver of disagreement between them was a band of ledge heights
//! the graph offered and the body then refused, and the walker stood at that
//! ledge forever (`libs/render/src/level.rs:216`, the E1M1 courtyard rim).
//!
//! This crate has one clearance oracle, `analysis::ClearanceField`. These
//! tests assert the properties that make it safe to have only one:
//!
//! * whatever the planner calls walkable, the oracle agrees is clear;
//! * whatever the oracle calls clear, the QA limit accepts;
//! * the oracle is never *optimistic* — it never reports more room than there
//!   really is, because a pessimistic planner makes dull films and an
//!   optimistic one flies through walls.

use makepad_fab_tour::analysis::ClearMode;
use makepad_fab_tour::*;
use makepad_math::vec3;

#[test]
fn walkable_and_the_oracle_never_disagree() {
    let scene = synthetic::villa();
    let site = SiteAnalysis::analyse(&scene, &AnalysisConfig::default());
    let radius = site.config.body.radius;

    for (si, st) in site.storeys.iter().enumerate() {
        let field = site.clearance(ClearMode::Walk(si));
        let mut checked = 0usize;
        for y in 0..st.ny {
            for x in 0..st.nx {
                let i = st.at(x, y);
                if !st.walkable[i] {
                    continue;
                }
                let w = site.grid.world_of(x, y, 0);
                let p = vec3(w.x, w.y, st.eye_z);
                let c = field.at(p);
                // The mask says a body fits; the oracle must not disagree.
                // One cell of slack for the bilinear filter, which averages
                // the cell with its neighbours.
                assert!(
                    c >= radius - site.grid.cell,
                    "storey {si} cell ({x},{y}) is walkable but the oracle \
                     reports {c:.3} m < radius {radius:.3} m — this is exactly \
                     the band the Doom walker got stuck in"
                );
                checked += 1;
            }
        }
        assert!(checked > 100, "storey {si}: only {checked} walkable cells");
    }
}

#[test]
fn the_oracle_is_never_optimistic() {
    // A single box: clearance at a known distance must never exceed the truth.
    let mut b = TourSceneBuilder::new("box");
    b.storey("g", 0.0, 3.0);
    b.element("w", TourClass::Wall, 0);
    b.box_solid(vec3(-2.0, -2.0, -2.0), vec3(2.0, 2.0, 2.0));
    let scene = b.finish();
    let site = SiteAnalysis::analyse(
        &scene,
        &AnalysisConfig {
            voxel: VoxelConfig {
                cell: 0.1,
                pad: 6.0,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let field = site.clearance(ClearMode::Fly);
    for d in [0.3f32, 0.7, 1.5, 3.0] {
        let p = vec3(2.0 + d, 0.0, 0.0);
        let c = field.at(p);
        assert!(
            c <= d + 1e-3,
            "at {d} m from the face the oracle claims {c:.3} m of room"
        );
        assert!(c > d - 0.2, "at {d} m the oracle is uselessly pessimistic: {c:.3}");
    }
    // On the surface there is no room at all. (Not *inside* it: voxelisation
    // marks triangles, so a closed solid is a shell — see the "Surfaces, not
    // solids" note in `voxel`, and why it cannot produce a phantom room.)
    assert!(field.at(vec3(2.0, 0.0, 0.0)) < 0.06);
    assert!(field.at(vec3(0.0, 2.0, 0.0)) < 0.06);
}

#[test]
fn qa_reads_the_same_field_the_planner_relaxed_against() {
    let scene = synthetic::villa();
    let site = SiteAnalysis::analyse(&scene, &AnalysisConfig::default());
    let track = shots::walkthrough(&site, &ShotOptions::default());
    assert!(!track.keys.is_empty());
    let limits = QaLimits::default();
    let report = qa::check(&site, &track, &limits);

    // The QA's reported minimum must equal an independent sweep of the very
    // same oracle. If these ever differ, someone has grown a second clearance
    // function and the fuse is lit.
    let fly = site.clearance(ClearMode::Fly);
    let mut min = f32::INFINITY;
    let n = ((track.duration() / limits.dt).ceil() as usize).max(2);
    for i in 0..=n {
        let t = (i as f32 * limits.dt).min(track.duration());
        if let Some(k) = track.sample(t) {
            min = min.min(fly.at(k.pos));
        }
    }
    assert!(
        (min - report.min_clearance).abs() < 1e-5,
        "QA says {:.4} m, an independent sweep says {min:.4} m",
        report.min_clearance
    );
    assert!(report.passed(), "{}", report.summary());
}

#[test]
fn string_pull_never_shortcuts_through_a_wall() {
    let scene = synthetic::villa();
    let site = SiteAnalysis::analyse(&scene, &AnalysisConfig::default());
    let radius = site.config.body.radius;
    let rooms = site.rooms_by_rank();
    assert!(rooms.len() >= 2);

    let mut routed = 0;
    for w in rooms.windows(2).take(6) {
        let (a, b) = (w[0], w[1]);
        if site.rooms[a].storey != site.rooms[b].storey {
            continue;
        }
        let si = site.rooms[a].storey;
        let Some(pts) = route::route_points(&site, si, site.rooms[a].center, site.rooms[b].center)
        else {
            continue;
        };
        let field = site.clearance(ClearMode::Walk(si));
        for seg in pts.windows(2) {
            assert!(
                field.segment_clear(seg[0], seg[1], radius),
                "string pull produced a leg through geometry: {:?} → {:?}",
                seg[0],
                seg[1]
            );
        }
        routed += 1;
    }
    assert!(routed > 0, "no same-storey room pairs were routed");
}
