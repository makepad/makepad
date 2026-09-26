//! The read-only inspection walk: order, depth, starts, labels, tracks.

use makepad_tween::*;

const OPACITY: PropKey = PropKey(1);
const SHIFT: PropKey = PropKey(2);
const INTRO: Tag = Tag(11);
const OUTRO: Tag = Tag(12);
const SHOW: Tag = Tag(13);

#[test]
fn inspect_walks_a_timeline_parent_first_with_local_starts() {
    let mut e = TweenEngine::new();
    for i in 0..2 {
        e.seed(TargetId(i), OPACITY, 0.0.into());
        e.seed(TargetId(i), SHIFT, 10.0.into());
    }
    let tl = e.timeline(TimelineOpts::new().tag(SHOW));
    e.tl(tl)
        .add_label(INTRO, 0.0)
        .to(
            TargetId(0).into(),
            &[PropTo::to_f64(OPACITY, 1.0), PropTo::to_f64(SHIFT, 0.0)],
            TweenOpts::new().duration(0.5),
            INTRO,
        )
        .add_label(OUTRO, Position::rel(0.25))
        .to(
            TargetId(1).into(),
            &[PropTo::to_f64(OPACITY, 1.0)],
            TweenOpts::new().duration(0.3),
            OUTRO,
        );

    let mut nodes = Vec::new();
    e.inspect(&mut nodes);
    assert_eq!(nodes.len(), 3, "the timeline and its two tweens");

    let top = nodes[0];
    assert_eq!(top.id, tl);
    assert_eq!(top.parent, TweenId::NONE);
    assert_eq!(top.depth, 0);
    assert_eq!(top.kind, InspectKind::Timeline);
    assert_eq!(top.tag, SHOW);
    assert_eq!(top.local_start, 0.0);
    assert!((top.duration - 1.05).abs() < 1e-9);

    let (a, b) = (nodes[1], nodes[2]);
    assert_eq!((a.parent, a.depth, a.kind), (tl, 1, InspectKind::Tween));
    assert_eq!((b.parent, b.depth), (tl, 1));
    assert_eq!(a.local_start, 0.0);
    assert!(
        (b.local_start - 0.75).abs() < 1e-9,
        "starts at the outro label"
    );
    assert_eq!(a.tracks, 2);
    assert_eq!(a.first_track, Some((TargetId(0), OPACITY)));
    assert_eq!(b.first_track, Some((TargetId(1), OPACITY)));

    let mut labels = Vec::new();
    e.inspect_labels(tl, &mut labels);
    assert_eq!(labels.len(), 2);
    assert_eq!(labels[0], (INTRO, 0.0));
    assert_eq!(labels[1].0, OUTRO);
    assert!((labels[1].1 - 0.75).abs() < 1e-9);

    let mut tracks = Vec::new();
    e.inspect_tracks(a.id, &mut tracks);
    assert_eq!(tracks, vec![(TargetId(0), OPACITY), (TargetId(0), SHIFT)]);

    // Read-only: the playhead has not moved.
    assert_eq!(e.anim_ref(tl).total_time(), 0.0);
}

#[test]
fn inspect_reports_a_stale_handle_as_nothing() {
    let mut e = TweenEngine::new();
    let mut labels = vec![(Tag(1), 1.0)];
    let mut tracks = vec![(TargetId(1), OPACITY)];
    e.inspect_labels(TweenId::NONE, &mut labels);
    e.inspect_tracks(TweenId::NONE, &mut tracks);
    assert!(labels.is_empty() && tracks.is_empty());
    let mut nodes = vec![];
    e.inspect(&mut nodes);
    assert!(nodes.is_empty(), "an engine that never built has no nodes");
    e.seed(TargetId(0), OPACITY, 0.0.into());
    e.inspect(&mut nodes);
    assert!(nodes.is_empty());
}

#[test]
fn inspect_keeps_listing_a_completed_kept_timeline() {
    let mut e = TweenEngine::new();
    e.seed(TargetId(0), OPACITY, 0.0.into());
    let tl = e.timeline(TimelineOpts::new().keep(true));
    e.tl(tl).to(
        TargetId(0).into(),
        &[PropTo::to_f64(OPACITY, 1.0)],
        TweenOpts::new().duration(0.25),
        0.0,
    );
    let mut nodes = Vec::new();
    e.inspect(&mut nodes);
    assert!(nodes[0].linked);

    while e.is_active() {
        e.advance(1.0 / 60.0);
        e.clear_events();
    }
    e.inspect(&mut nodes);
    assert_eq!(nodes.len(), 2, "the completed timeline and its tween");
    assert_eq!((nodes[0].id, nodes[0].depth), (tl, 0));
    assert!(!nodes[0].linked, "detached from the root once complete");
    assert!(nodes[1].linked, "still on its own timeline");
    assert!((nodes[0].total_time - 0.25).abs() < 1e-9);

    e.anim(tl).restart(false, Emit::Suppress);
    e.inspect(&mut nodes);
    assert_eq!(nodes.len(), 2, "listed once after a restart");
    assert!(nodes[0].linked);
}
