//! Plain data: position strings, option builders and inheritance, masks,
//! targets, ids and the crate-level helpers.

use makepad_tween::{
    animation_cycle, round7, splitmix64, Anchor, Easing, End, EventMask, Offset, Overwrite,
    Position, PositionError, PropKey, PropTo, Reduce, Stagger, Tag, TargetId, Targets,
    TimelineOpts, TweenId, TweenOpts, TweenValue, YoyoEase, BIG, INFINITE,
};

/// Label names map to tags by a fixed table in these tests.
fn tag(name: &str) -> Tag {
    match name {
        "intro" => Tag(1),
        "outro" => Tag(2),
        other => panic!("unexpected label {other:?}"),
    }
}

#[test]
fn position_parse_accepts_every_gsap_form() {
    let intro = Tag(1);
    let cases = [
        ("2", Position::at(2.0)),
        ("-0.5", Position::at(-0.5)),
        ("+=1", Position::rel(1.0)),
        ("-=0.5", Position::rel(-0.5)),
        ("+=50%", Position::rel_percent(50.0)),
        ("-=50%", Position::rel_percent(-50.0)),
        ("<", Position::prev_start(0.0)),
        ("<0.5", Position::prev_start(0.5)),
        ("<-0.5", Position::prev_start(-0.5)),
        ("<+=0.5", Position::prev_start(0.5)),
        ("<-=0.5", Position::prev_start(-0.5)),
        ("<25%", Position::prev_start_percent(25.0)),
        ("<+=25%", Position::prev_start_percent_of_child(25.0)),
        (">", Position::prev_end(0.0)),
        (">0.2", Position::prev_end(0.2)),
        (">-0.2", Position::prev_end(-0.2)),
        (">50%", Position::prev_end_percent(50.0)),
        (">-50%", Position::prev_end_percent(-50.0)),
        (">-=50%", Position::prev_end_percent_of_child(-50.0)),
        ("intro", Position::label(intro)),
        ("intro+=1", Position::label_rel(intro, 1.0)),
        ("intro-=1", Position::label_rel(intro, -1.0)),
        ("intro+=50%", Position::label_rel_percent(intro, 50.0)),
        (" outro ", Position::label(Tag(2))),
    ];
    for (s, want) in cases {
        assert_eq!(Position::parse(s, tag), Ok(want), "{s:?}");
    }
    let errors = [
        ("", PositionError::Empty),
        ("   ", PositionError::Empty),
        ("+=x", PositionError::BadNumber),
        ("<<", PositionError::BadNumber),
        ("<+=", PositionError::BadNumber),
        (">%", PositionError::BadNumber),
        ("intro+=", PositionError::BadNumber),
        ("inf", PositionError::BadNumber),
        ("=1", PositionError::BadForm),
        ("intro*=2", PositionError::BadForm),
        ("<*=2", PositionError::BadForm),
    ];
    for (s, want) in errors {
        assert_eq!(Position::parse(s, tag), Err(want), "{s:?}");
    }
}

#[test]
fn position_constructors_and_conversions() {
    assert_eq!(Position::default(), Position::END);
    assert_eq!(
        Position::END,
        Position {
            anchor: Anchor::End,
            offset: Offset::Secs(0.0)
        }
    );
    assert_eq!(Position::from(1.5), Position::at(1.5));
    assert_eq!(Position::from(Tag(9)), Position::label(Tag(9)));
    assert_eq!(
        Position::prev_start_percent(25.0).offset,
        Offset::PercentOfPrev(25.0)
    );
    assert_eq!(
        Position::prev_end_percent_of_child(10.0).anchor,
        Anchor::PrevEnd
    );
}

#[test]
fn tween_opts_builders_are_const_and_inherit_fieldwise() {
    const OPEN: TweenOpts = TweenOpts::new()
        .duration(0.25)
        .ease(Easing::OutCubic)
        .repeat(2)
        .yoyo(true)
        .tag(Tag(5))
        .on_start()
        .on_complete();
    assert_eq!(OPEN.duration, Some(0.25));
    assert_eq!(OPEN.events, EventMask::START.with(EventMask::COMPLETE));
    assert_eq!(TweenOpts::new(), TweenOpts::default());

    let parent = TweenOpts::new()
        .duration(1.0)
        .delay(0.5)
        .ease(Easing::Linear)
        .ease_each(Easing::InOutQuad)
        .yoyo_ease(YoyoEase::Invert)
        .repeat_delay(0.1)
        .repeat_refresh(true)
        .stagger(Stagger::each(0.1))
        .overwrite(Overwrite::Auto)
        .immediate_render(true)
        .paused(true)
        .reversed(true)
        .time_scale(2.0)
        .color_space(makepad_tween::ColorSpace::Oklch)
        .reduce(Reduce::Freeze)
        .keep(true)
        .inherit(false)
        .tag(Tag(7))
        .events(EventMask::ALL);
    let merged = OPEN.or(&parent);
    assert_eq!(merged.duration, Some(0.25), "own value wins");
    assert_eq!(merged.ease, Some(Easing::OutCubic));
    assert_eq!(merged.delay, Some(0.5), "unset inherits");
    assert_eq!(merged.overwrite, Some(Overwrite::Auto));
    assert_eq!(merged.stagger, Some(Stagger::each(0.1)));
    assert_eq!(merged.reduce, Some(Reduce::Freeze));
    assert_eq!(merged.tag, Tag(5), "tag is never inherited");
    assert_eq!(merged.events, OPEN.events, "events are never inherited");
    assert_eq!(TweenOpts::new().or(&parent).tag, Tag::NONE);
    let all = TweenOpts::new()
        .on_update()
        .on_repeat()
        .on_reverse_complete()
        .on_interrupt()
        .on_start()
        .on_complete();
    assert_eq!(
        all.events,
        EventMask(EventMask::ALL.0 & !EventMask::LABELS.0)
    );
}

#[test]
fn timeline_opts_defaults_and_builders() {
    let d = TimelineOpts::default();
    assert_eq!(d, TimelineOpts::new());
    assert_eq!(d.time_scale, 1.0);
    assert!(d.keep);
    assert!(!d.smooth_child_timing && !d.auto_remove_children && !d.paused && !d.yoyo);
    assert_eq!((d.repeat, d.delay, d.repeat_delay), (0, 0.0, 0.0));
    const TL: TimelineOpts = TimelineOpts::new()
        .defaults(TweenOpts::new().duration(0.3))
        .delay(1.0)
        .repeat(-1)
        .repeat_delay(0.5)
        .repeat_refresh(true)
        .yoyo(true)
        .paused(true)
        .reversed(true)
        .time_scale(0.5)
        .smooth_child_timing(true)
        .auto_remove_children(true)
        .keep(false)
        .reduce(Reduce::Keep)
        .tag(Tag(3))
        .events(EventMask::START)
        .watch_labels()
        .on_update()
        .on_repeat()
        .on_complete()
        .on_reverse_complete()
        .on_interrupt()
        .on_start();
    assert_eq!(TL.defaults.duration, Some(0.3));
    assert_eq!(TL.repeat, -1);
    assert!(!TL.keep);
    assert_eq!(TL.events, EventMask::ALL);
}

#[test]
fn event_mask_sets() {
    assert!(EventMask::ALL.has(EventMask::EDGES));
    assert!(EventMask::EDGES.has(EventMask::INTERRUPT));
    assert!(!EventMask::EDGES.has(EventMask::UPDATE));
    assert!(!EventMask::EDGES.has(EventMask::LABELS));
    assert!(EventMask::NONE.has(EventMask::NONE));
    assert_eq!(EventMask::default(), EventMask::NONE);
    assert_eq!(EventMask::START.with(EventMask::UPDATE), EventMask(3));
}

#[test]
fn prop_to_constructors() {
    const X: PropKey = PropKey(1);
    const KEYS: [makepad_tween::Key; 1] = [makepad_tween::Key {
        at: 50.0,
        value: TweenValue::F64(1.0),
        ease: None,
    }];
    const VALS: [TweenValue; 2] = [TweenValue::F64(0.0), TweenValue::F64(1.0)];
    assert_eq!(PropTo::to_f64(X, 2.0), PropTo::to(X, TweenValue::F64(2.0)));
    assert_eq!(PropTo::to(X, 2.0.into()).to, End::To(TweenValue::F64(2.0)));
    assert_eq!(PropTo::by(X, 1.0.into()).to, End::By(TweenValue::F64(1.0)));
    let from = PropTo::from(X, 5.0.into());
    assert_eq!(
        (from.from, from.to),
        (Some(TweenValue::F64(5.0)), End::Current)
    );
    let ft = PropTo::from_to(X, 1.0.into(), 2.0.into());
    assert_eq!(
        (ft.from, ft.to),
        (Some(TweenValue::F64(1.0)), End::To(TweenValue::F64(2.0)))
    );
    assert_eq!(PropTo::each(X, &VALS).to, End::Each(&VALS));
    assert_eq!(PropTo::values(X, &VALS).to, End::Values(&VALS));
    assert_eq!(PropTo::keys(X, &KEYS).to, End::Keys(&KEYS));
    assert_eq!(PropTo::to_f64(X, 1.0).snap(0.5).snap, 0.5);
    assert_eq!(PropTo::to_f64(X, 1.0).snap, 0.0);
}

#[test]
fn targets() {
    let list = [TargetId(4), TargetId(9)];
    let one: Targets = TargetId(3).into();
    assert_eq!((one.len(), one.get(0)), (1, TargetId(3)));
    let range = Targets::Range {
        first: 10,
        count: 3,
    };
    assert_eq!((range.len(), range.get(2)), (3, TargetId(12)));
    let listed: Targets = list.as_slice().into();
    assert_eq!((listed.len(), listed.get(1)), (2, TargetId(9)));
    assert!(Targets::List(&[]).is_empty());
    assert!(!one.is_empty());
}

#[test]
fn ids() {
    assert_eq!(PropKey::path(&[42]), PropKey(42));
    const NESTED: PropKey = PropKey::path(&[1, 2]);
    assert_eq!(NESTED, PropKey(splitmix64(splitmix64(1) ^ 2)));
    assert_ne!(PropKey::path(&[1, 2]), PropKey::path(&[2, 1]));
    assert_eq!(PropKey::path(&[]), PropKey(0));
    assert!(TweenId::NONE.is_none());
    assert_eq!(Tag::default(), Tag::NONE);
}

#[test]
fn round7_is_gsap_round_precise() {
    assert_eq!(round7(1.0 / 3.0), 0.3333333);
    assert_eq!(round7(0.123456789), 0.1234568);
    assert_eq!(round7(-1.0 / 3.0), -0.3333333);
    assert_eq!(round7(INFINITE), INFINITE);
    assert_eq!(
        round7(BIG + 0.123456789),
        BIG + 0.123456789,
        "identity from BIG up"
    );
}

#[test]
fn round7_rounds_ties_like_javascript() {
    // JS Math.round sends ties toward +infinity [golden: design_round7_negative_ties].
    assert_eq!(round7(-0.00000025).to_bits(), (-2e-7f64).to_bits());
    assert_eq!(round7(-0.00000015).to_bits(), (-1e-7f64).to_bits());
    assert_eq!(round7(0.00000025).to_bits(), 3e-7f64.to_bits());
    assert_eq!(round7(0.00000015).to_bits(), 2e-7f64.to_bits());
    // A zero result is +0 (GSAP `|| 0`).
    assert_eq!(round7(-1e-9).to_bits(), 0.0f64.to_bits());
    assert_eq!(round7(-0.0).to_bits(), 0.0f64.to_bits());
}

#[test]
fn animation_cycle_boundaries() {
    assert_eq!(animation_cycle(0.0, 1.0), 0);
    assert_eq!(animation_cycle(0.5, 1.0), 0);
    assert_eq!(
        animation_cycle(1.0, 1.0),
        0,
        "an exact boundary belongs to the ending iteration"
    );
    assert_eq!(animation_cycle(1.25, 1.0), 1);
    assert_eq!(animation_cycle(2.0, 1.0), 1);
    assert_eq!(animation_cycle(1.5, 1.5), 0);
    assert_eq!(animation_cycle(3.0 - 1e-9, 1.5), 1, "rounded to 1e-7 first");
    assert_eq!(animation_cycle(-1.0, 1.0), 0, "negative saturates");
}

#[test]
fn splitmix64_reference_values() {
    // The first outputs of the reference SplitMix64 generator seeded with 0
    // (state advances by the golden gamma before mixing).
    assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
    assert_eq!(splitmix64(0x9E37_79B9_7F4A_7C15), 0x6E78_9E6A_A1B9_65F4);
}
