//! The architecture overview layout on the repository's own plan.
use makepad_code_arch::{parse, Limits};
use makepad_director::architecture::plan::build_design;
use makepad_widgets::dvec2;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

#[test]
fn the_platform_plan_lays_out_deterministically_without_overlap() {
    let path = repo_root().join("arch/platform/crate.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("no platform plan at {}; skipped", path.display());
        return;
    };
    let plan = parse(&text, &Limits::default()).expect("the platform plan parses");
    let a = build_design(&plan);
    let b = build_design(&plan);
    assert_eq!(a, b, "the layout is deterministic");
    assert!(a.nodes.len() >= 8 && a.lanes.len() >= 2, "{} nodes, {} lanes", a.nodes.len(), a.lanes.len());
    for (i, n) in a.nodes.iter().enumerate() {
        assert!(n.rect.is_inside_of(a.bounds), "{} outside the bounds", n.id);
        if let Some(l) = a.lane_rect_of(i) {
            assert!(n.rect.is_inside_of(l), "{} outside its lane", n.id);
        }
        for m in a.nodes.iter().skip(i + 1) {
            assert!(!n.rect.intersects(m.rect), "{} overlaps {}", n.id, m.id);
        }
    }
    let mut routed = 0;
    for e in &a.edges {
        if e.from == e.to {
            continue;
        }
        assert!(e.points.len() >= 2, "edge {} unrouted", e.id);
        let (s, t) = (a.nodes[e.from].rect, a.nodes[e.to].rect);
        assert!(s.add_margin(dvec2(1.0, 1.0)).contains(e.points[0]), "edge {} does not leave its source", e.id);
        assert!(t.add_margin(dvec2(1.0, 1.0)).contains(*e.points.last().unwrap()), "edge {} does not reach its target", e.id);
        // orthogonal: every segment is horizontal or vertical
        for w in e.points.windows(2) {
            assert!((w[0].x - w[1].x).abs() < 1e-6 || (w[0].y - w[1].y).abs() < 1e-6, "edge {} has a diagonal segment", e.id);
        }
        routed += 1;
    }
    assert_eq!(routed, a.edges.iter().filter(|e| e.from != e.to).count());
    // the overview reads: the aspect is a landscape, not a strip
    let aspect = a.bounds.size.x / a.bounds.size.y.max(1.0);
    assert!(aspect > 0.6 && aspect < 4.0, "aspect {aspect:.2}");
    eprintln!("record=design_platform nodes={} edges={} lanes={} bounds={:.0}x{:.0} aspect={:.2}", a.nodes.len(), a.edges.len(), a.lanes.len(), a.bounds.size.x, a.bounds.size.y, aspect);
}
