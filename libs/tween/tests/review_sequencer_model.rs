//! Review regression tests for the Sequencer's engine model
//! (`makepad_tween::sequencer_model`: `SequencerModel::from_engine`,
//! `apply_sequencer_edit`, the drag snapping) and the engine accessors it
//! reads. Every test checks the model against the engine itself (global
//! times, rendered values), so a failure is a disagreement between what the
//! Sequencer draws or edits and what the engine plays. The drag tests run
//! the widget's own drag maths (`item_snap_targets` once at the press, then
//! `drag_move` / `drag_resize_end` per pointer move) against a host that
//! rebuilds the model after every applied edit, as the storybook page does.

mod util;

use makepad_tween::*;
use util::*;

type Action = SequencerAction;
type Kind = SequencerKind;

/// The model of `root`, every row and label named by its tag's Debug text.
fn model(e: &TweenEngine, root: TweenId) -> SequencerModel {
    SequencerModel::from_engine(e, root, |_, t| format!("{t:?}"))
}

fn apply(e: &mut TweenEngine, root: TweenId, a: Action) -> bool {
    apply_sequencer_edit(e, root, a)
}

const S: PropKey = PropKey(20);

/// A timeline-local time of `root` for an engine (root-level) time.
fn root_local(e: &TweenEngine, root: TweenId, g: f64) -> f64 {
    let r = e.anim_ref(root);
    (g - r.global_time(0.0)) * r.time_scale().abs()
}

/// Builds: root (time scale 1.5, a label), a delayed tween, a nested
/// timeline (time scale 2) holding a repeating tween and a nested-nested
/// timeline (time scale 0.5, delay 0.25), a call and a pause.
fn nested_world(e: &mut TweenEngine) -> TweenId {
    // Not paused: a paused animation's engine time scale is 0, and
    // `global_time` (the oracle) then divides by 1 (GSAP `|_ts| || 1`). It
    // is never advanced.
    let root = e.timeline(TimelineOpts::new().keep(true).time_scale(1.5));
    e.tl(root).add_label(Tag(7), 0.4).to(
        one(0),
        &[to(X, 1.0)],
        lin(0.5).delay(0.2).tag(Tag(1)),
        0.3,
    );
    let nested = e.timeline(TimelineOpts::new().time_scale(2.0).tag(Tag(2)));
    e.tl(nested).to(
        one(1),
        &[to(X, 1.0)],
        lin(1.0).repeat(2).repeat_delay(0.1).tag(Tag(3)),
        1.0,
    );
    let nn = e.timeline(TimelineOpts::new().time_scale(0.5).delay(0.25).tag(Tag(4)));
    e.tl(nn)
        .to(one(2), &[to(X, 1.0)], lin(0.4).tag(Tag(5)), 0.2);
    e.tl(nested).add(nn, 0.5);
    e.tl(root)
        .add(nested, 1.0)
        .call(Tag(8), 2.0)
        .add_pause(2.5, Tag(9));
    root
}

#[test]
fn nested_rows_sit_where_the_engine_plays_them() {
    let mut e = TweenEngine::new();
    let root = nested_world(&mut e);
    let m = model(&e, root);
    assert_eq!(m.tracks.len(), 8, "{m:#?}");
    for t in &m.tracks[1..] {
        let it = &t.items[0];
        let a = e.anim_ref(TweenId::from_bits(it.id));
        let g0 = a.global_time(0.0);
        let g1 = a.global_time(a.duration());
        close(
            it.start,
            root_local(&e, root, g0),
            1e-9,
            &format!("{} start", t.name),
        );
        close(
            it.start + it.duration,
            root_local(&e, root, g1),
            1e-9,
            &format!("{} end", t.name),
        );
    }
    // Kinds, depths, locks, markers.
    let kinds: Vec<(Kind, u32, bool)> = m
        .tracks
        .iter()
        .map(|t| (t.kind, t.depth, t.items[0].locked))
        .collect();
    assert_eq!(
        kinds,
        vec![
            (Kind::Timeline, 0, true),
            (Kind::Tween, 1, false),
            (Kind::Timeline, 1, false),
            // Start order inside the nested timeline: the delayed inner
            // timeline (local 0.75) before the repeating tween (local 1.0).
            (Kind::Timeline, 2, false),
            (Kind::Tween, 3, false),
            (Kind::Tween, 2, false),
            (Kind::Call, 1, false),
            (Kind::Pause, 1, false),
        ],
        "{m:#?}"
    );
}

/// Every unlocked row: a move and a resize land where they were asked to
/// (the model read back equals the request), and the root answers `false`.
#[test]
fn moves_and_resizes_round_trip() {
    let mut e = TweenEngine::new();
    let root = nested_world(&mut e);
    let m = model(&e, root);
    assert!(!apply(
        &mut e,
        root,
        Action::ItemMoved {
            id: root.to_bits(),
            start: 1.0
        }
    ));
    for t in &m.tracks[1..] {
        let it = &t.items[0];
        if it.locked {
            continue;
        }
        let want = it.start + 0.125;
        assert!(apply(
            &mut e,
            root,
            Action::ItemMoved {
                id: it.id,
                start: want
            }
        ));
        let got = model(&e, root);
        close(
            got.item(it.id).unwrap().start,
            want,
            2e-7,
            &format!("{} moved", t.name),
        );
        if it.duration > 0.0 {
            let want = it.duration * 1.25;
            assert!(apply(
                &mut e,
                root,
                Action::ItemResized {
                    id: it.id,
                    duration: want
                }
            ));
            let got = model(&e, root);
            close(
                got.item(it.id).unwrap().duration,
                want,
                2e-7,
                &format!("{} resized", t.name),
            );
        }
    }
}

/// A repeating nested timeline with a repeat delay: the bar the user
/// dragged to 2.0 s reads back 2.0 s after the host re-syncs, and its repeat
/// gap scales with it (the edit sets the time scale directly; GSAP's
/// `duration(v)` would spread a new TOTAL over repeats whose delay is in the
/// timeline's local time, and the bar would jump to 1.8 s).
#[test]
fn resizing_a_repeating_nested_timeline_keeps_the_dragged_length() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    let nested = e.timeline(TimelineOpts::new().repeat(1).repeat_delay(0.5));
    e.tl(nested).to(one(0), &[to(X, 1.0)], lin(1.0), 0.0);
    e.tl(root).add(nested, 0.0);
    let before = model(&e, root);
    close(
        before.item(nested.to_bits()).unwrap().duration,
        1.0,
        1e-12,
        "before",
    );
    assert!(apply(
        &mut e,
        root,
        Action::ItemResized {
            id: nested.to_bits(),
            duration: 2.0
        }
    ));
    let after = model(&e, root);
    let it = after.item(nested.to_bits()).unwrap();
    close(
        it.duration,
        2.0,
        1e-9,
        "the nested timeline's bar after the resize",
    );
    close(it.repeat_delay, 1.0, 1e-9, "its repeat gap, scaled with it");
    // The engine agrees: the second iteration starts after 2.0 + 1.0 s.
    let a = e.anim_ref(nested);
    close(a.global_time(0.0), 0.0, 1e-9, "start");
    close(
        a.end_time(true),
        2.0 * 2.0 + 1.0,
        1e-9,
        "end with the repeat",
    );
}

/// A stretched stagger group: its members' rows sit where the engine plays
/// them (a member is half-way at the middle of its row).
#[test]
fn a_resized_stagger_group_places_its_members_where_they_play() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root).from_to(
        Targets::Range { first: 0, count: 4 },
        &[PropTo::from_to(S, 0.0.into(), 1.0.into())],
        lin(0.4).stagger(Stagger::each(0.1)),
        0.5,
    );
    let m = model(&e, root);
    let group = m
        .tracks
        .iter()
        .find(|t| t.kind == Kind::Group)
        .unwrap()
        .items[0]
        .clone();
    for resize in [None, Some(group.duration * 2.0)] {
        if let Some(d) = resize {
            assert!(apply(
                &mut e,
                root,
                Action::ItemResized {
                    id: group.id,
                    duration: d
                }
            ));
        }
        let m = model(&e, root);
        let members: Vec<_> = m
            .tracks
            .iter()
            .filter(|t| t.depth == 2)
            .map(|t| t.items[0].clone())
            .collect();
        assert_eq!(members.len(), 4);
        for (i, it) in members.iter().enumerate() {
            let target = e
                .anim_ref(TweenId::from_bits(it.id))
                .first_target()
                .unwrap();
            e.anim(root)
                .seek(Seek::Time(it.start + 0.5 * it.duration), Emit::Suppress);
            close(
                e.get_f64(target, S).unwrap(),
                0.5,
                1e-6,
                &format!("member {i} at the middle of its row (resize {resize:?})"),
            );
        }
    }
}

/// Array keyframes: each step's row is where the engine plays it.
#[test]
fn keyframe_steps_sit_where_they_play() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    let a = [PropTo::from_to(S, 0.0.into(), 1.0.into())];
    let b = [PropTo::from_to(S, 10.0.into(), 11.0.into())];
    e.tl(root).keyframes(
        one(0),
        &[
            KeyStep {
                props: &a,
                opts: lin(0.5),
            },
            KeyStep {
                props: &b,
                opts: lin(1.0).delay(0.25),
            },
        ],
        TweenOpts::new(),
        0.3,
    );
    let m = model(&e, root);
    let steps: Vec<_> = m
        .tracks
        .iter()
        .filter(|t| t.depth == 2)
        .map(|t| t.items[0].clone())
        .collect();
    assert_eq!(steps.len(), 2, "{m:#?}");
    for (i, (it, base)) in steps.iter().zip([0.0, 10.0]).enumerate() {
        e.anim(root)
            .seek(Seek::Time(it.start + 0.5 * it.duration), Emit::Suppress);
        close(
            e.get_f64(tg(0), S).unwrap(),
            base + 0.5,
            1e-6,
            &format!("step {i} at the middle of its row"),
        );
    }
}

/// Builds the same timeline with explicit placements (a fresh engine), and
/// returns the values at `t`.
fn fresh(a_start: f64, a_dur: f64, b_start: f64, b_dur: f64, c_start: f64, t: f64) -> [f64; 2] {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root)
        .from_to(
            one(0),
            &[PropTo::from_to(X, 0.0.into(), 100.0.into())],
            lin(a_dur).tag(Tag(1)),
            a_start,
        )
        .from_to(
            one(0),
            &[PropTo::from_to(X, 200.0.into(), 300.0.into())],
            lin(b_dur).tag(Tag(2)),
            b_start,
        );
    let nested = e.timeline(TimelineOpts::new().time_scale(2.0));
    e.tl(nested).from_to(
        one(1),
        &[PropTo::from_to(Y, 0.0.into(), 10.0.into())],
        lin(1.0),
        c_start,
    );
    e.tl(root).add(nested, 0.5);
    e.anim(root).seek(Seek::Time(t), Emit::Suppress);
    [val(&e, 0, X), val(&e, 1, Y)]
}

/// After an edit at a fixed playhead, `refresh` shows what a fresh build
/// with the edited placement shows at that time.
#[test]
fn refresh_matches_a_fresh_seek() {
    let t = 1.5;
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root)
        .from_to(
            one(0),
            &[PropTo::from_to(X, 0.0.into(), 100.0.into())],
            lin(2.0).tag(Tag(1)),
            0.0,
        )
        .from_to(
            one(0),
            &[PropTo::from_to(X, 200.0.into(), 300.0.into())],
            lin(2.0).tag(Tag(2)),
            3.0,
        );
    let nested = e.timeline(TimelineOpts::new().time_scale(2.0));
    e.tl(nested).from_to(
        one(1),
        &[PropTo::from_to(Y, 0.0.into(), 10.0.into())],
        lin(1.0),
        0.0,
    );
    e.tl(root).add(nested, 0.5);
    e.anim(root).seek(Seek::Time(t), Emit::Suppress);
    let m = model(&e, root);
    let id_of = |tag: u64| {
        m.tracks
            .iter()
            .find(|tr| tr.name == format!("{:?}", Tag(tag)))
            .unwrap()
            .id
    };
    let (ia, ib) = (id_of(1), id_of(2));
    let ic = m.tracks.iter().find(|tr| tr.depth == 2).unwrap().id;
    assert_eq!(
        [val(&e, 0, X), val(&e, 1, Y)],
        fresh(0.0, 2.0, 3.0, 2.0, 0.0, t)
    );
    // B moves under the playhead (after A in start order: B wins).
    assert!(apply(
        &mut e,
        root,
        Action::ItemMoved { id: ib, start: 1.0 }
    ));
    assert_eq!(
        [val(&e, 0, X), val(&e, 1, Y)],
        fresh(0.0, 2.0, 1.0, 2.0, 0.0, t),
        "B moved to 1.0"
    );
    // A moves after B (now B then A: A wins).
    assert!(apply(
        &mut e,
        root,
        Action::ItemMoved { id: ia, start: 1.2 }
    ));
    assert_eq!(
        [val(&e, 0, X), val(&e, 1, Y)],
        fresh(1.2, 2.0, 1.0, 2.0, 0.0, t),
        "A moved to 1.2"
    );
    // B stretches.
    assert!(apply(
        &mut e,
        root,
        Action::ItemResized {
            id: ib,
            duration: 4.0
        }
    ));
    assert_eq!(
        [val(&e, 0, X), val(&e, 1, Y)],
        fresh(1.2, 2.0, 1.0, 4.0, 0.0, t),
        "B resized to 4.0"
    );
    // The nested tween moves (model time 0.5 + 0.2 = local 0.4 at scale 2).
    assert!(apply(
        &mut e,
        root,
        Action::ItemMoved { id: ic, start: 0.7 }
    ));
    assert_eq!(
        [val(&e, 0, X), val(&e, 1, Y)],
        fresh(1.2, 2.0, 1.0, 4.0, 0.4, t),
        "the nested tween moved"
    );
}

#[test]
fn edits_that_must_be_refused() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root).add_label(Tag(3), 0.5).from_to(
        Targets::Range { first: 0, count: 3 },
        &[PropTo::from_to(S, 0.0.into(), 1.0.into())],
        lin(0.4).stagger(Stagger::each(0.1)),
        0.0,
    );
    let other = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(other).to(one(9), &[to(X, 1.0)], lin(1.0), 0.0);
    let foreign = e.anim_ref(other).children().next().unwrap();
    let m = model(&e, root);
    let member = m.tracks.iter().find(|t| t.depth == 2).unwrap().items[0].id;
    let before = m.clone();
    for a in [
        Action::ItemMoved {
            id: member,
            start: 0.9,
        },
        Action::ItemResized {
            id: member,
            duration: 0.9,
        },
        Action::ItemMoved {
            id: foreign.to_bits(),
            start: 0.9,
        },
        Action::ItemMoved {
            id: 0xdead_0000_0001,
            start: 0.9,
        },
        Action::ItemResized {
            id: m.tracks[1].id,
            duration: 0.0,
        },
        Action::ItemResized {
            id: m.tracks[1].id,
            duration: -1.0,
        },
        Action::LabelMoved { id: 4, time: 0.9 },
        Action::ItemMoved {
            id: m.tracks[1].id,
            start: f64::NAN,
        },
    ] {
        assert!(!apply(&mut e, root, a), "{a:?}");
    }
    assert_eq!(model(&e, root), before);
    // A label moved past the end is placed there; the duration does not grow.
    assert!(apply(&mut e, root, Action::LabelMoved { id: 3, time: 9.0 }));
    let m = model(&e, root);
    let labels: Vec<(u64, f64)> = m.labels.iter().map(|l| (l.id, l.time)).collect();
    assert_eq!(labels, vec![(3, 9.0)]);
    close(m.duration, before.duration, 1e-12, "duration");
}

/// A killed member of a group is not a row, and the group's caption and
/// `has_children` agree with the rows.
#[test]
fn killed_children_leave_no_row() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root).from_to(
        Targets::Range { first: 0, count: 3 },
        &[PropTo::from_to(S, 0.0.into(), 1.0.into())],
        lin(0.4).stagger(Stagger::each(0.1)),
        0.0,
    );
    e.kill_tweens_of(one(1), None);
    let m = model(&e, root);
    let members = m.tracks.iter().filter(|t| t.depth == 2).count();
    assert_eq!(members, 2, "{m:#?}");
    let group = m.tracks.iter().find(|t| t.kind == Kind::Group).unwrap();
    assert_eq!(group.items[0].label, "stagger x2");
}

/// The playhead stays inside the model after an edit shortens a timeline
/// that sat at its end.
#[test]
fn the_playhead_stays_inside_a_shortened_timeline() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root)
        .from_to(
            one(0),
            &[PropTo::from_to(X, 0.0.into(), 1.0.into())],
            lin(1.0),
            0.0,
        )
        .from_to(
            one(1),
            &[PropTo::from_to(X, 0.0.into(), 1.0.into())],
            lin(1.0),
            2.0,
        );
    e.anim(root).seek(Seek::Time(3.0), Emit::Suppress);
    let m = model(&e, root);
    let last = m.tracks[2].items[0].id;
    assert!(apply(
        &mut e,
        root,
        Action::ItemMoved {
            id: last,
            start: 0.5
        }
    ));
    let m = model(&e, root);
    assert!(
        m.playhead <= m.duration + 1e-9,
        "playhead {} beyond duration {}",
        m.playhead,
        m.duration
    );
}

// ---------------------------------------------------------------------------
// The widget's Move drag against a re-syncing host
// ---------------------------------------------------------------------------

/// Drags item `id` by `steps` pointer moves of one pixel each (a real
/// mouse), as the widget's Move drag does: the snap targets are taken once
/// at the press, every move asks `drag_move`, and the host applies the edit
/// and rebuilds the model after every `ItemMoved` (the storybook's recipe).
/// Returns the largest distance in pixels between where the pointer puts
/// the bar and where the bar is.
fn drag_lag_px(e: &mut TweenEngine, root: TweenId, id: u64, zoom: f64, steps: u32) -> f64 {
    let radius = 6.0 / zoom; // snap_px 6
    let mut m = model(e, root);
    let mut targets = Vec::new();
    m.item_snap_targets(id, m.playhead, &mut targets);
    let start0 = m.item(id).unwrap().start;
    let mut worst: f64 = 0.0;
    for k in 1..=steps {
        let raw = start0 + k as f64 / zoom; // t_at(x) - grab_dt
        let d = m.drag_move(id, raw, &targets, radius).unwrap();
        if d.start != m.item(id).unwrap().start
            && apply(e, root, Action::ItemMoved { id, start: d.start })
        {
            m = model(e, root);
        }
        let now = m.item(id).unwrap().start;
        worst = worst.max((raw - now).abs() * zoom);
    }
    worst
}

/// The right-edge twin of [`drag_lag_px`]: the first iteration's end
/// follows the pointer through `drag_resize_end`.
fn resize_lag_px(e: &mut TweenEngine, root: TweenId, id: u64, zoom: f64, steps: u32) -> f64 {
    let radius = 6.0 / zoom;
    let mut m = model(e, root);
    let mut targets = Vec::new();
    m.item_snap_targets(id, m.playhead, &mut targets);
    let it = m.item(id).unwrap();
    let end0 = it.start + it.duration;
    let mut worst: f64 = 0.0;
    for k in 1..=steps {
        let raw = end0 + k as f64 / zoom;
        let d = m.drag_resize_end(id, raw, &targets, radius, 0.01).unwrap();
        if d.duration != m.item(id).unwrap().duration
            && apply(
                e,
                root,
                Action::ItemResized {
                    id,
                    duration: d.duration,
                },
            )
        {
            m = model(e, root);
        }
        let it = m.item(id).unwrap();
        worst = worst.max((raw - (it.start + it.duration)).abs() * zoom);
    }
    worst
}

/// The storybook page's shape: a title, a nested timeline of three cards,
/// a middle tween, and a last tween that defines the duration.
fn story_world(e: &mut TweenEngine) -> (TweenId, u64, u64, u64) {
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root).from_to(
        one(0),
        &[PropTo::from_to(X, 0.0.into(), 1.0.into())],
        lin(0.6),
        0.0,
    );
    let cards = e.timeline(TimelineOpts::new());
    for i in 0..3 {
        e.tl(cards).from_to(
            one(10 + i),
            &[PropTo::from_to(X, 40.0.into(), 0.0.into())],
            lin(0.3),
            Position::END,
        );
    }
    e.tl(root)
        .add(cards, 0.7)
        .from_to(
            one(1),
            &[PropTo::from_to(X, 0.0.into(), 1.0.into())],
            lin(0.5),
            1.9,
        )
        .from_to(
            one(2),
            &[PropTo::from_to(X, 0.0.into(), 1.0.into())],
            lin(0.8),
            2.8,
        );
    let kids: Vec<TweenId> = e.anim_ref(root).children().collect();
    (root, cards.to_bits(), kids[2].to_bits(), kids[3].to_bits())
}

/// Control: a middle tween far from every other edge follows the pointer
/// pixel for pixel.
#[test]
fn a_middle_bar_follows_the_pointer() {
    let mut e = TweenEngine::new();
    let (root, _, middle, _) = story_world(&mut e);
    let lag = drag_lag_px(&mut e, root, middle, 168.75, 30);
    // set_start_time rounds to 1e-7 s (round7): 1.7e-5 px at this zoom.
    assert!(lag < 1e-3, "middle bar lags {lag} px");
}

/// A nested timeline's own children ride with it after every re-sync: they
/// are not snap targets (the targets are taken at the press, the dragged
/// subtree left out), so the bar follows the pointer pixel for pixel
/// instead of sticking to its own children and trailing by up to 6 px.
#[test]
fn a_nested_timeline_bar_follows_the_pointer() {
    let mut e = TweenEngine::new();
    let (root, cards, _, _) = story_world(&mut e);
    let lag = drag_lag_px(&mut e, root, cards, 168.75, 30);
    assert!(
        lag < 1.0,
        "the nested timeline's bar lags the pointer by {lag:.2} px"
    );
}

/// The last bar defines the model duration (and the root row's end): ends
/// the dragged bar defines are not snap targets, so it follows the pointer.
#[test]
fn the_last_bar_follows_the_pointer() {
    let mut e = TweenEngine::new();
    let (root, _, _, last) = story_world(&mut e);
    let lag = drag_lag_px(&mut e, root, last, 168.75, 30);
    assert!(lag < 1.0, "the last bar lags the pointer by {lag:.2} px");
}

/// A reversed nested timeline plays its children mirrored and backwards:
/// each child's row covers exactly the time it plays in, running from its
/// end at the row's left edge to its start at the right edge, and the item
/// says it runs backwards.
#[test]
fn a_reversed_nested_timeline_places_its_children_where_they_play() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    let nested = e.timeline(TimelineOpts::new().reversed(true));
    e.tl(nested)
        .from_to(
            one(0),
            &[PropTo::from_to(S, 0.0.into(), 1.0.into())],
            lin(1.0),
            0.0,
        )
        .from_to(
            one(1),
            &[PropTo::from_to(S, 0.0.into(), 1.0.into())],
            lin(1.0),
            1.0,
        );
    e.tl(root).add(nested, 0.0);
    let m = model(&e, root);
    let kids: Vec<&SequencerTrack> = m.tracks.iter().filter(|t| t.depth == 2).collect();
    assert_eq!(kids.len(), 2, "{m:#?}");
    assert!(m.item(nested.to_bits()).unwrap().reversed);
    // The first child (local 0..1) plays last: its row is 1..2.
    close(kids[0].items[0].start, 1.0, 1e-9, "the first child's row");
    close(kids[1].items[0].start, 0.0, 1e-9, "the second child's row");
    for t in kids {
        let it = &t.items[0];
        assert!(it.reversed, "{} runs backwards in model time", t.name);
        let target = e
            .anim_ref(TweenId::from_bits(it.id))
            .first_target()
            .unwrap();
        for (f, want) in [(0.25, 0.75), (0.5, 0.5), (0.75, 0.25)] {
            e.anim(root)
                .seek(Seek::Time(it.start + f * it.duration), Emit::Suppress);
            let v = e.get_f64(target, S).unwrap();
            close(
                v,
                want,
                1e-6,
                &format!("target {target:?} {f} into its row"),
            );
        }
    }
}

/// Edits inside a reversed nested timeline land where the child then plays:
/// a move puts the row's left edge where it was dropped, a resize keeps the
/// left edge (the local start is re-placed, mirrored).
#[test]
fn a_reversed_nested_timeline_child_moves_and_resizes_where_it_plays() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    let nested = e.timeline(TimelineOpts::new().reversed(true));
    for i in 0..3 {
        e.tl(nested).from_to(
            one(i),
            &[PropTo::from_to(S, 0.0.into(), 1.0.into())],
            lin(1.0),
            i as f64,
        );
    }
    e.tl(root).add(nested, 0.0);
    let m = model(&e, root);
    // The middle child (local 1..2) plays at 1..2 as well (3 - 2 = 1).
    let mid = m
        .tracks
        .iter()
        .filter(|t| t.depth == 2)
        .find(|t| e.anim_ref(TweenId::from_bits(t.id)).first_target() == Some(tg(1)))
        .unwrap()
        .items[0]
        .clone();
    close(mid.start, 1.0, 1e-9, "the middle child's row");
    assert!(apply(
        &mut e,
        root,
        Action::ItemMoved {
            id: mid.id,
            start: 1.5
        }
    ));
    let it = model(&e, root).item(mid.id).unwrap().clone();
    close(it.start, 1.5, 1e-9, "moved");
    assert!(apply(
        &mut e,
        root,
        Action::ItemResized {
            id: mid.id,
            duration: 0.5
        }
    ));
    let it = model(&e, root).item(mid.id).unwrap().clone();
    close(it.start, 1.5, 1e-9, "the left edge stays");
    close(it.duration, 0.5, 1e-9, "resized");
    // And the engine plays it there, backwards.
    for (f, want) in [(0.25, 0.75), (0.75, 0.25)] {
        e.anim(root)
            .seek(Seek::Time(it.start + f * it.duration), Emit::Suppress);
        close(
            val(&e, 1, S),
            want,
            1e-6,
            &format!("{f} into the moved row"),
        );
    }
}

/// The resize twin of the drag tests: the last bar's right edge (which
/// defines the duration and the root's end) follows the pointer.
#[test]
fn the_last_bar_right_edge_follows_the_pointer() {
    let mut e = TweenEngine::new();
    let (root, cards, _, last) = story_world(&mut e);
    let lag = resize_lag_px(&mut e, root, last, 168.75, 30);
    assert!(
        lag < 1.0,
        "the last bar's edge lags the pointer by {lag:.2} px"
    );
    let lag = resize_lag_px(&mut e, root, cards, 168.75, 30);
    assert!(
        lag < 1.0,
        "the nested timeline's edge lags the pointer by {lag:.2} px"
    );
}

/// What a drag snaps to: taken at the press, without the dragged bar's own
/// subtree or the ends it defines, and with everything else.
#[test]
fn drag_targets_leave_out_what_the_dragged_bar_defines() {
    let mut e = TweenEngine::new();
    let (root, cards, middle, last) = story_world(&mut e);
    let m = model(&e, root);
    let mut t = Vec::new();
    let has = |t: &[f64], x: f64| t.iter().any(|v| (v - x).abs() < 1e-9);
    // Cards (0.7 + 0.9, children at 0.7, 1.0, 1.3): its children are out,
    // the others and the duration are in.
    m.item_snap_targets(cards, 0.25, &mut t);
    assert!(
        has(&t, 0.0) && has(&t, 0.25) && has(&t, m.duration),
        "{t:?}"
    );
    assert!(has(&t, 0.6), "the title's end: {t:?}");
    assert!(!has(&t, 1.0) && !has(&t, 1.3) && !has(&t, 1.6), "{t:?}");
    assert!(!has(&t, 0.7), "its own start: {t:?}");
    // The last bar (2.8 + 0.8) defines the duration and the root's end.
    m.item_snap_targets(last, 0.0, &mut t);
    assert!(!has(&t, 3.6) && !has(&t, 2.8), "{t:?}");
    assert!(has(&t, 1.9) && has(&t, 2.4), "the middle bar: {t:?}");
    // A middle bar keeps the duration as a target.
    m.item_snap_targets(middle, 0.0, &mut t);
    assert!(has(&t, 3.6) && has(&t, 2.8) && !has(&t, 1.9), "{t:?}");
    // snap_time: the nearest target within the radius, else the time.
    assert_eq!(snap_time(&[1.0, 2.0], 1.04, 0.05), 1.0);
    assert_eq!(snap_time(&[1.0, 2.0], 1.06, 0.05), 1.06);
    assert_eq!(snap_time(&[1.0, 1.1], 1.06, 0.05), 1.1);
    assert_eq!(snap_time(&[], 1.06, 0.05), 1.06);
    // A move snaps its end too, and never goes before its parent.
    let d = m.drag_move(middle, 2.29, &[2.8], 0.02).unwrap();
    close(d.start, 2.3, 1e-12, "the end (2.29 + 0.5) snapped to 2.8");
    assert_eq!(d.guide, Some(2.8));
    let d = m.drag_move(middle, -1.0, &[], 0.0).unwrap();
    assert_eq!(d.start, 0.0);
}

/// A timeline with an `add_pause` before the playhead: an edit's refresh
/// renders 0 -> playhead and must not stop at the pause (a seek passes it).
#[test]
fn refresh_passes_add_pause() {
    let mut e = TweenEngine::new();
    let root = e.timeline(TimelineOpts::new().paused(true).keep(true));
    e.tl(root)
        .from_to(
            one(0),
            &[PropTo::from_to(X, 0.0.into(), 300.0.into())],
            lin(3.0),
            0.0,
        )
        .add_pause(1.0, Tag(5));
    e.anim(root).seek(Seek::Time(2.0), Emit::Suppress);
    close(val(&e, 0, X), 200.0, 1e-9, "seek passes the pause");
    let m = model(&e, root);
    let tween = m.tracks.iter().find(|t| t.kind == Kind::Tween).unwrap().id;
    assert!(apply(
        &mut e,
        root,
        Action::ItemMoved {
            id: tween,
            start: 0.5
        }
    ));
    close(
        e.anim_ref(root).total_time(),
        2.0,
        1e-9,
        "the playhead after the edit",
    );
    close(
        val(&e, 0, X),
        150.0,
        1e-9,
        "x at the playhead after the edit",
    );
}
