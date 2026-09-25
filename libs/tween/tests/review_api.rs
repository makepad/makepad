//! Adversarial review, lens: API, robustness and quality.
//!
//! - every public item of design section 2 exists with the spec signature,
//! - the builders are `const fn` (used in `const` items below),
//! - `Position::parse` grammar and error cases,
//! - `Stagger::delay == delays()[i]` bit for bit (random / grid / edges, odd n),
//! - `QuickTo` against a `UnitTween` oracle over long seeded runs,
//! - value conversions (hue range, hue borrowing, alpha, clamping, u32),
//! - `Easing` `PartialEq` / `Default` / `Debug`,
//! - the API used the way a widget would (the crate-level example).

use makepad_tween::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::marker::PhantomData;

// ---------------------------------------------------------------------------
// Per-thread allocation counter
// ---------------------------------------------------------------------------

struct Counting;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    let _ = ARMED.try_with(|a| {
        if a.get() {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        }
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note();
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        note();
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// How many allocations this thread made inside `f`.
fn allocations<R>(f: impl FnOnce() -> R) -> (u64, R) {
    COUNT.with(|c| c.set(0));
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (COUNT.with(|c| c.get()), r)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const OPACITY: PropKey = PropKey(1);
const SHIFT: PropKey = PropKey(2);
const FILL: PropKey = PropKey(3);
const INTRO: Tag = Tag(11);
const OUTRO: Tag = Tag(12);

/// A small deterministic generator (SplitMix64 from the crate).
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(1);
        splitmix64(self.0)
    }

    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

/// Answers whether `T: Default` without failing to compile when it is not
/// (autoref specialisation).
struct Probe<T>(PhantomData<T>);

trait HasDefault {
    fn has_default(&self) -> bool {
        true
    }
}

impl<T: Default> HasDefault for Probe<T> {}

// Unused while every probed type implements Default (the fallback arm).
#[allow(dead_code)]
trait LacksDefault {
    fn has_default(&self) -> bool {
        false
    }
}

impl<T> LacksDefault for &Probe<T> {}

#[track_caller]
fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: got {a:?}, want {b:?} (tol {tol:e})"
    );
}

// ---------------------------------------------------------------------------
// 1. The public surface of design section 2, with the spec signatures
// ---------------------------------------------------------------------------

#[test]
fn every_section_2_item_exists_with_the_spec_signature() {
    // 2.1 lib.rs
    let _: f64 = INFINITE + BIG + TINY;
    let _: fn(f64) -> f64 = round7;
    let _: fn(f64, f64) -> u32 = animation_cycle;
    const _SM: u64 = splitmix64(1);

    // 2.2 ids.rs
    const _PK: PropKey = PropKey::path(&[1, 2, 3]);
    let _: Tag = Tag::NONE;
    let _: TweenId = TweenId::NONE;
    let _: fn(TweenId) -> bool = TweenId::is_none;
    let _: SlotId = SlotId(0);
    let _: TargetId = TargetId::default();

    // 2.3 value.rs
    const _RGBA: Rgba = Rgba::new(0.0, 0.0, 0.0, 1.0);
    let _: fn(u32) -> Rgba = Rgba::from_u32;
    let _: fn(Rgba) -> u32 = Rgba::to_u32;
    let _: fn([f32; 4]) -> Rgba = Rgba::from_f32;
    let _: fn(Rgba) -> [f32; 4] = Rgba::to_f32;
    let _: fn(&TweenValue) -> ValueKind = TweenValue::kind;
    let _: fn(&TweenValue) -> [f64; 4] = TweenValue::to_lanes;
    let _: fn(ValueKind, [f64; 4]) -> TweenValue = TweenValue::from_lanes;
    let _: fn(&TweenValue) -> f64 = TweenValue::as_f64;
    let _: fn(&TweenValue, &TweenValue, f64, ColorSpace) -> TweenValue = TweenValue::lerp;
    let _: [TweenValue; 6] = [
        1.0f64.into(),
        [1.0, 2.0].into(),
        [1.0, 2.0, 3.0].into(),
        [1.0, 2.0, 3.0, 4.0].into(),
        3i64.into(),
        Rgba::default().into(),
    ];
    let _: fn(f64) -> f64 = srgb_to_linear;
    let _: fn(f64) -> f64 = linear_to_srgb;
    let _: fn([f64; 3]) -> [f64; 3] = rgb_to_hsv;
    let _: fn([f64; 3]) -> [f64; 3] = hsv_to_rgb;
    let _: fn([f64; 3]) -> [f64; 3] = linear_to_oklab;
    let _: fn([f64; 3]) -> [f64; 3] = oklab_to_linear;
    let _: fn([f64; 3]) -> [f64; 3] = oklab_to_oklch;
    let _: fn([f64; 3]) -> [f64; 3] = oklch_to_oklab;
    let _: fn(Rgba, ColorSpace) -> [f64; 4] = encode;
    let _: fn([f64; 4], ColorSpace) -> Rgba = decode;
    let _: ValueKind = ValueKind::default();
    let _: ColorSpace = ColorSpace::default();

    // 2.4 easing.rs
    let _: fn(&Easing, f64) -> f64 = Easing::map;
    let _: fn(f64, f64, usize) -> Easing = Easing::exp_decay;
    let _: fn(f64, f64) -> Easing = Easing::pow;
    let _: fn(f64, f64, f64, f64) -> Easing = Easing::css;
    let _: fn(u8, EaseDir) -> Easing = Easing::power;
    let _: fn(&str) -> Option<Easing> = Easing::css_preset;
    let _: fn(&Easing) -> Option<[f64; 4]> = Easing::css_points;
    let _: [(&str, [f64; 4]); 25] = CSS_PRESETS;
    let _: fn(&str) -> Option<Easing> = parse_gsap_ease;
    let _ = [Jump::Start, Jump::End, Jump::Both, Jump::None];
    let _ = YoyoEase::Ease(Easing::Linear);
    let _ = Easing::Custom(CustomEase(|t| t));

    // 2.5 spec.rs
    let list = [TargetId(1), TargetId(2)];
    let t: Targets = TargetId(0).into();
    let _: u32 = t.len();
    let _: TargetId = Targets::List(&list).get(1);
    let _ = Targets::Range { first: 0, count: 3 };
    let _ = [
        End::To(1.0.into()),
        End::By(1.0.into()),
        End::Current,
        End::Each(&[]),
        End::Keys(&[]),
        End::Values(&[]),
    ];
    let _ = [Overwrite::None, Overwrite::Auto, Overwrite::All];
    let _ = [Reduce::JumpToEnd, Reduce::Freeze, Reduce::Keep];
    let _ = [Emit::Suppress, Emit::Fire];
    let _ = KeyStep {
        props: &[],
        opts: TweenOpts::new(),
    };
    let _: fn(TweenOpts, &TweenOpts) -> TweenOpts = TweenOpts::or;
    let _: fn(&str, fn(&str) -> Tag) -> Result<Position, PositionError> = Position::parse;
    let _: Position = 1.0.into();
    let _: Position = Tag(1).into();
    let _: Position = Position::default();
    let _ = [
        Seek::Time(0.0),
        Seek::Label(Tag(1), 0.0),
        Seek::Progress(0.0),
        Seek::TotalProgress(0.0),
    ];
    let _ = [
        PositionError::Empty,
        PositionError::BadNumber,
        PositionError::BadForm,
    ];

    // 2.6 stagger.rs
    let _: fn(&Stagger, u32, &mut [f64]) = Stagger::delays;
    let _: fn(&Stagger, u32, u32) -> f64 = Stagger::delay;
    let _: fn(&Stagger, u32) -> f64 = Stagger::step;
    let _ = StaggerGrid { rows: 1, cols: 1 };
    let _ = [StaggerAxis::X, StaggerAxis::Y];

    // 2.7 event.rs
    let _ = [
        EventKind::Start,
        EventKind::Update,
        EventKind::Repeat,
        EventKind::Complete,
        EventKind::ReverseComplete,
        EventKind::Interrupt,
        EventKind::Call { forward: true },
        EventKind::Pause,
        EventKind::Label(Tag(1)),
    ];
    let _: Stats = Stats::default();

    // 2.8 engine.rs / control.rs
    let _: fn() -> TweenEngine = TweenEngine::new;
    let _: fn(usize, usize, usize) -> TweenEngine = TweenEngine::with_capacity;
    let _: fn(&mut TweenEngine, TweenOpts) = TweenEngine::set_defaults;
    let _: fn(&TweenEngine) -> TweenOpts = TweenEngine::defaults;
    type Build = fn(&mut TweenEngine, Targets<'_>, &[PropTo<'_>], TweenOpts) -> TweenId;
    let _: [Build; 5] = [
        TweenEngine::tween,
        TweenEngine::to,
        TweenEngine::from,
        TweenEngine::from_to,
        TweenEngine::set,
    ];
    let _: fn(&mut TweenEngine, Targets<'_>, &[KeyStep<'_>], TweenOpts) -> TweenId =
        TweenEngine::keyframes;
    let _: fn(&mut TweenEngine, f64, Tag) -> TweenId = TweenEngine::delayed_call;
    let _: fn(&mut TweenEngine, TimelineOpts) -> TweenId = TweenEngine::timeline;
    let _: fn(&mut TweenEngine, TweenId) -> TimelineMut<'_> = TweenEngine::tl;
    let _: fn(&mut TweenEngine, TweenId) -> AnimMut<'_> = TweenEngine::anim;
    let _: fn(&TweenEngine, TweenId) -> AnimRef<'_> = TweenEngine::anim_ref;
    let _: fn(&mut TweenEngine, Targets<'_>, Option<&[PropKey]>) = TweenEngine::kill_tweens_of;
    let _: fn(&mut TweenEngine) = TweenEngine::kill_all;
    let _: fn(&TweenEngine, TargetId) -> bool = TweenEngine::is_tweening;
    let _: fn(&TweenEngine, Tag) -> Option<TweenId> = TweenEngine::get_by_tag;
    let _: fn(&TweenEngine) -> f64 = TweenEngine::root_time_scale;
    let _: fn(&mut TweenEngine, f64) = TweenEngine::set_root_time_scale;
    let _: fn(&mut TweenEngine, f64) = TweenEngine::advance;
    let _: fn(&TweenEngine) -> bool = TweenEngine::is_active;
    let _: fn(&mut TweenEngine, TweenId) = TweenEngine::finish;
    let _: fn(&mut TweenEngine) = TweenEngine::finish_all;
    let _: fn(&mut TweenEngine, TargetId, PropKey, TweenValue) -> SlotId = TweenEngine::seed;
    let _: fn(&TweenEngine, TargetId, PropKey) -> Option<SlotId> = TweenEngine::slot;
    let _: fn(&TweenEngine, SlotId) -> TweenValue = TweenEngine::value;
    let _: fn(&TweenEngine, TargetId, PropKey) -> Option<TweenValue> = TweenEngine::get;
    let _: fn(&TweenEngine, TargetId, PropKey) -> Option<f64> = TweenEngine::get_f64;
    let _: fn(&TweenEngine, SlotId) -> (TargetId, PropKey) = TweenEngine::slot_key;
    let _: fn(&TweenEngine) -> u32 = TweenEngine::slot_count;
    let _: fn(&TweenEngine) -> u32 = TweenEngine::slot_generation;
    let _: fn(&TweenEngine) -> &[SlotId] = TweenEngine::changes;
    let _: fn(&TweenEngine, SlotId) -> bool = TweenEngine::is_changed;
    let _: fn(&mut TweenEngine) = TweenEngine::clear_changes;
    let _: fn(&mut TweenEngine) = TweenEngine::mark_all_changed;
    let _: fn(&TweenEngine, TweenId, SlotId, f64) -> Option<TweenValue> = TweenEngine::sample;
    let _: fn(&TweenEngine) -> &[TweenEvent] = TweenEngine::events;
    let _: fn(&mut TweenEngine, &mut Vec<TweenEvent>) = TweenEngine::swap_events;
    let _: fn(&mut TweenEngine) = TweenEngine::clear_events;
    let _: fn(&TweenEngine) -> Stats = TweenEngine::stats;
    let _: TweenEngine = TweenEngine::default();

    // TimelineMut / AnimMut / AnimRef, called on a live engine.
    let mut e = TweenEngine::new();
    e.seed(TargetId(0), OPACITY, 0.0.into());
    let child = e.timeline(TimelineOpts::new());
    let tl = e.timeline(TimelineOpts::new().paused(true));
    let props = [PropTo::to_f64(OPACITY, 1.0)];
    let steps = [KeyStep {
        props: &props,
        opts: TweenOpts::new().duration(0.1),
    }];
    {
        let mut b: TimelineMut<'_> = e.tl(tl);
        let r: &mut TimelineMut<'_> = b
            .tween(TargetId(0).into(), &props, TweenOpts::new(), Position::END)
            .to(TargetId(0).into(), &props, TweenOpts::new(), 1.0)
            .from(
                TargetId(0).into(),
                &[PropTo::from(OPACITY, 0.5.into())],
                TweenOpts::new(),
                Tag(1),
            )
            .from_to(
                TargetId(0).into(),
                &[PropTo::from_to(OPACITY, 0.0.into(), 1.0.into())],
                TweenOpts::new(),
                Position::rel(0.1),
            )
            .set(TargetId(0).into(), &props, TweenOpts::new(), Position::END)
            .keyframes(TargetId(0).into(), &steps, TweenOpts::new(), Position::END)
            .add(child, Position::END)
            .add_label(Tag(2), Position::END)
            .remove_label(Tag(2))
            .call(Tag(3), 0.5)
            .add_pause(0.75, Tag(4))
            .shift_children(0.0, false, 0.0);
        let _: TweenId = r.last();
        let _: TweenId = r.id();
        r.remove(child).clear(false);
    }
    {
        let mut a: AnimMut<'_> = e.anim(tl);
        let _: &mut AnimMut<'_> = a
            .play()
            .play_from(Seek::Time(0.0), Emit::Suppress)
            .pause()
            .pause_at(Seek::Progress(0.5), Emit::Fire)
            .resume()
            .reverse()
            .reverse_from(None, Emit::Suppress)
            .restart(false, Emit::Suppress)
            .seek(Seek::TotalProgress(0.0), Emit::Suppress)
            .set_progress(0.0, Emit::Fire)
            .set_total_progress(0.0, Emit::Fire)
            .set_time(0.0, Emit::Fire)
            .set_total_time(0.0, Emit::Fire)
            .set_iteration(1, Emit::Fire)
            .set_time_scale(1.0)
            .set_reversed(false)
            .set_paused(true)
            .set_duration(1.0)
            .set_delay(0.0)
            .set_start_time(0.0)
            .set_repeat(0)
            .set_repeat_delay(0.0)
            .set_yoyo(false)
            .invalidate();
    }
    {
        let r: AnimRef<'_> = e.anim_ref(tl);
        let _: [bool; 5] = [
            r.is_alive(),
            r.is_active(),
            r.paused(),
            r.reversed(),
            r.yoyo(),
        ];
        let _: [f64; 11] = [
            r.time(),
            r.total_time(),
            r.progress(),
            r.total_progress(),
            r.duration(),
            r.total_duration(),
            r.time_scale(),
            r.start_time(),
            r.end_time(true),
            r.delay(),
            r.global_time(0.0),
        ];
        let _: u32 = r.iteration();
        let _: i32 = r.repeat();
        let _: Tag = r.tag();
        let _: [Option<Tag>; 3] = [r.current_label(), r.next_label(), r.previous_label()];
        let _: Option<f64> = r.label_time(Tag(1));
        let _: u32 = r.child_count();
    }
    e.anim(child).kill();

    // 2.9 quick.rs
    let mut q: QuickTo<f64> = QuickTo::at(0.0);
    let _: bool = q.aim(1.0, 0.5, Easing::Linear, Retime::Full);
    let _: bool = q.aim(
        0.0,
        0.5,
        Easing::Linear,
        Retime::ByDistance { per_unit: 1.0 },
    );
    q.restart(1.0, 0.5, Easing::Linear);
    q.retarget_from(0.0, 1.0, 0.5, Easing::Linear);
    let _: bool = q.step(0.1);
    let _: f64 = q.value() + q.from_value() + q.target() + q.progress() + q.duration();
    let _: Easing = q.ease();
    let _: bool = q.is_settled();
    q.settle();
    let _: QuickTo<[f64; 4]> = QuickTo::at([0.0; 4]);

    // 2.10 ticker.rs
    const _LAG: LagSmoothing = LagSmoothing::clamp(0.1);
    let _ = [LagSmoothing::GSAP, LagSmoothing::OFF];
    let _: fn(LagSmoothing, f64) -> f64 = LagSmoothing::filter;
    let _: fn(&TweenTicker, Option<f64>, ClockPolicy) -> f64 = TweenTicker::dt;
    let _ = (TweenTicker::default(), ClockPolicy::default());
}

/// Every builder the spec marks `const fn` is usable in a `const` item.
#[test]
fn builders_are_const_fn() {
    const PROPS: [PropTo<'static>; 7] = [
        PropTo::to(OPACITY, TweenValue::F64(1.0)),
        PropTo::to_f64(OPACITY, 1.0),
        PropTo::by(OPACITY, TweenValue::F64(1.0)),
        PropTo::from(OPACITY, TweenValue::F64(0.0)),
        PropTo::from_to(OPACITY, TweenValue::F64(0.0), TweenValue::F64(1.0)).snap(0.5),
        PropTo::each(OPACITY, &[TweenValue::F64(1.0)]),
        PropTo::values(OPACITY, &[TweenValue::F64(1.0)]),
    ];
    const KEYS: [Key; 1] = [Key {
        at: 50.0,
        value: TweenValue::F64(1.0),
        ease: None,
    }];
    const KEYED: PropTo<'static> = PropTo::keys(OPACITY, &KEYS);
    const STAGGER: Stagger = Stagger::each(0.1)
        .from(StaggerFrom::Center)
        .grid(2, 3)
        .axis(StaggerAxis::X)
        .ease(Easing::InQuad)
        .base(0.5)
        .each_repeat(1)
        .each_yoyo(true)
        .each_repeat_delay(0.1);
    const STAGGER2: [Stagger; 2] = [Stagger::amount(1.0), Stagger::each_within(0.1, 0.5)];
    const OPEN: TweenOpts = TweenOpts::new()
        .duration(0.25)
        .delay(0.1)
        .ease(Easing::OutCubic)
        .ease_each(Easing::Linear)
        .yoyo_ease(YoyoEase::Invert)
        .repeat(2)
        .repeat_delay(0.1)
        .repeat_refresh(true)
        .yoyo(true)
        .stagger(STAGGER)
        .overwrite(Overwrite::Auto)
        .immediate_render(false)
        .paused(false)
        .reversed(false)
        .time_scale(2.0)
        .color_space(ColorSpace::Oklch)
        .reduce(Reduce::Freeze)
        .keep(true)
        .inherit(true)
        .tag(Tag(9))
        .events(EventMask::NONE)
        .on_start()
        .on_update()
        .on_repeat()
        .on_complete()
        .on_reverse_complete()
        .on_interrupt();
    const TL: TimelineOpts = TimelineOpts::new()
        .defaults(OPEN)
        .delay(0.5)
        .repeat(-1)
        .repeat_delay(0.1)
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
        .events(EventMask::COMPLETE)
        .watch_labels();
    const POS: [Position; 13] = [
        Position::END,
        Position::at(1.0),
        Position::rel(1.0),
        Position::rel_percent(50.0),
        Position::prev_start(0.5),
        Position::prev_start_percent(25.0),
        Position::prev_start_percent_of_child(25.0),
        Position::prev_end(0.2),
        Position::prev_end_percent(50.0),
        Position::prev_end_percent_of_child(50.0),
        Position::label(INTRO),
        Position::label_rel(INTRO, 1.0),
        Position::label_rel_percent(INTRO, 50.0),
    ];
    const MASK: EventMask = EventMask::START.with(EventMask::COMPLETE);
    const HAS: bool = MASK.has(EventMask::COMPLETE);

    assert_eq!(PROPS[4].snap, 0.5);
    assert_eq!(KEYED.to, End::Keys(&KEYS));
    assert_eq!(STAGGER.grid, Some(StaggerGrid { rows: 2, cols: 3 }));
    assert_eq!(
        STAGGER2[1].spread,
        Spread::EachWithin {
            each: 0.1,
            amount: 0.5
        }
    );
    assert_eq!(OPEN.events, EventMask(1 | 2 | 4 | 8 | 16 | 32));
    assert!(TL.events.has(EventMask::LABELS) && TL.events.has(EventMask::COMPLETE));
    assert_eq!(POS[0], Position::default());
    assert!(HAS);
}

// ---------------------------------------------------------------------------
// 2. Position::parse
// ---------------------------------------------------------------------------

fn label_of(name: &str) -> Tag {
    match name {
        "intro" => INTRO,
        "outro" => OUTRO,
        other => Tag(1000 + other.len() as u64),
    }
}

#[track_caller]
fn parses(s: &str, want: Position) {
    assert_eq!(Position::parse(s, label_of), Ok(want), "parse({s:?})");
}

#[track_caller]
fn rejects(s: &str, want: PositionError) {
    assert_eq!(Position::parse(s, label_of), Err(want), "parse({s:?})");
}

#[test]
fn position_parse_every_grammar_form() {
    // numbers (JS `+position` accepts signs, exponents and surrounding space)
    parses("2", Position::at(2.0));
    parses("-1.5", Position::at(-1.5));
    parses("+3", Position::at(3.0));
    parses("1e1", Position::at(10.0));
    parses(" 0.25 ", Position::at(0.25));
    parses(".5", Position::at(0.5));
    // relative to the end
    parses("+=1", Position::rel(1.0));
    parses("-=1", Position::rel(-1.0));
    parses("+=50%", Position::rel_percent(50.0));
    parses("-=50%", Position::rel_percent(-50.0));
    // previous start / end
    parses("<", Position::prev_start(0.0));
    parses("<0.5", Position::prev_start(0.5));
    parses("<-0.5", Position::prev_start(-0.5));
    parses("<+=0.5", Position::prev_start(0.5));
    parses("<-=0.5", Position::prev_start(-0.5));
    parses("<25%", Position::prev_start_percent(25.0));
    parses("<-25%", Position::prev_start_percent(-25.0));
    parses("<+=25%", Position::prev_start_percent_of_child(25.0));
    parses("<-=25%", Position::prev_start_percent_of_child(-25.0));
    parses(">", Position::prev_end(0.0));
    parses(">0.2", Position::prev_end(0.2));
    parses(">-0.2", Position::prev_end(-0.2));
    parses(">+=0.2", Position::prev_end(0.2));
    parses(">-=0.2", Position::prev_end(-0.2));
    parses(">50%", Position::prev_end_percent(50.0));
    parses(">+=50%", Position::prev_end_percent_of_child(50.0));
    parses(">-=50%", Position::prev_end_percent_of_child(-50.0));
    // labels
    parses("intro", Position::label(INTRO));
    parses("intro+=1", Position::label_rel(INTRO, 1.0));
    parses("intro-=1", Position::label_rel(INTRO, -1.0));
    parses("intro+=50%", Position::label_rel_percent(INTRO, 50.0));
    parses("intro-=50%", Position::label_rel_percent(INTRO, -50.0));
    // a label whose name ends in % keeps it (GSAP: isNaN("50%") -> label)
    parses("50%", Position::label(label_of("50%")));
    // a label that looks like a unit ("0.5s") is a label, as in GSAP
    parses("0.5s", Position::label(label_of("0.5s")));
}

#[test]
fn position_parse_rejects_malformed_strings_without_panicking() {
    rejects("", PositionError::Empty);
    rejects("   ", PositionError::Empty);
    rejects("+=", PositionError::BadNumber);
    rejects("-=x", PositionError::BadNumber);
    rejects("+=1e400", PositionError::BadNumber);
    rejects("<abc", PositionError::BadNumber);
    rejects("<%", PositionError::BadNumber);
    rejects("<+=", PositionError::BadNumber);
    rejects("intro+=", PositionError::BadNumber);
    rejects("intro+=%", PositionError::BadNumber);
    rejects("=1", PositionError::BadForm);
    rejects("intro*=2", PositionError::BadForm);
    rejects("<*=2", PositionError::BadForm);
    // Non-ASCII input around the operators must not slice inside a char.
    for s in [
        "é=1",
        "<é",
        ">é%",
        "é+=1",
        "<+=é",
        "ラベル+=1",
        "ラベル",
        "<\u{1F600}",
        "\u{1F600}=",
        "+=\u{1F600}%",
    ] {
        let _ = Position::parse(s, label_of);
    }
}

// ---------------------------------------------------------------------------
// 3. Stagger::delay == delays()[i], bit for bit
// ---------------------------------------------------------------------------

fn stagger_matrix() -> Vec<Stagger> {
    let froms = [
        StaggerFrom::Index(0),
        StaggerFrom::Index(2),
        StaggerFrom::Index(40),
        StaggerFrom::Start,
        StaggerFrom::Center,
        StaggerFrom::Edges,
        StaggerFrom::End,
        StaggerFrom::Ratio(0.25, 0.75),
        StaggerFrom::Random(0),
        StaggerFrom::Random(7),
        StaggerFrom::Random(u64::MAX),
    ];
    let spreads = [
        Stagger::each(0.1),
        Stagger::each(-0.07),
        Stagger::amount(1.0),
        Stagger::each_within(0.05, 0.3),
    ];
    let mut out = Vec::new();
    for s in spreads {
        for f in froms {
            let base = s.from(f);
            out.push(base);
            out.push(base.ease(Easing::InQuad));
            out.push(base.ease(Easing::OutBack).base(0.25));
            out.push(base.grid(3, 3));
            out.push(base.grid(2, 5).axis(StaggerAxis::X));
            out.push(
                base.grid(5, 2)
                    .axis(StaggerAxis::Y)
                    .ease(Easing::InOutCubic),
            );
            out.push(base.grid(1, 7));
        }
    }
    out
}

#[test]
fn stagger_delay_equals_delays_bit_for_bit_at_odd_n() {
    for s in stagger_matrix() {
        for n in [1u32, 3, 5, 7, 9, 11, 13, 25, 31] {
            let mut all = vec![0.0; n as usize];
            s.delays(n, &mut all);
            for i in 0..n {
                let one = s.delay(i, n);
                assert_eq!(
                    one.to_bits(),
                    all[i as usize].to_bits(),
                    "{s:?} n={n} i={i}: delay {one} vs delays {}",
                    all[i as usize]
                );
            }
        }
    }
}

#[test]
fn stagger_random_is_a_permutation_of_the_ordered_delays() {
    for seed in [0u64, 1, 42, u64::MAX] {
        for n in [3u32, 7, 13, 31] {
            let mut ordered = vec![0.0; n as usize];
            Stagger::each(0.1)
                .from(StaggerFrom::Start)
                .delays(n, &mut ordered);
            let mut shuffled = vec![0.0; n as usize];
            Stagger::each(0.1)
                .from(StaggerFrom::Random(seed))
                .delays(n, &mut shuffled);
            let mut a = ordered.clone();
            let mut b = shuffled.clone();
            a.sort_by(f64::total_cmp);
            b.sort_by(f64::total_cmp);
            for (x, y) in a.iter().zip(&b) {
                close(*x, *y, 1e-7, "random stagger multiset");
            }
        }
    }
}

/// A degenerate grid must not produce NaN start times (a NaN start poisons
/// every sibling after it in a timeline).
#[test]
fn stagger_degenerate_grid_gives_finite_delays() {
    for s in [
        Stagger::each(0.1).grid(0, 0),
        Stagger::each(0.1).grid(3, 0),
        Stagger::amount(1.0).grid(0, 0).from(StaggerFrom::Center),
    ] {
        let mut out = vec![0.0; 5];
        s.delays(5, &mut out);
        for (i, d) in out.iter().enumerate() {
            assert!(d.is_finite(), "{s:?}: delay[{i}] = {d}");
            assert_eq!(d.to_bits(), s.delay(i as u32, 5).to_bits());
        }
    }
}

// ---------------------------------------------------------------------------
// 4. QuickTo against the UnitTween oracle (widgets/src/pill_nav.rs:1195)
// ---------------------------------------------------------------------------

/// widgets' `UnitTween`, verbatim except `Ease` -> `Easing` (the parity arms
/// are bit-identical to `Ease::map`, pinned by tests/easing.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
struct UnitTween {
    from: f64,
    to: f64,
    t: f64,
    secs: f64,
    ease: Easing,
    peak: f64,
}

impl UnitTween {
    fn at(value: f64) -> Self {
        Self {
            from: value,
            to: value,
            t: 1.0,
            secs: 0.0,
            ease: Easing::Linear,
            peak: value.clamp(0.0, 1.0),
        }
    }

    fn aim(&mut self, to: f64, secs: f64, ease: Easing) {
        if to == self.to {
            return;
        }
        let now = self.value();
        self.secs = secs.max(0.0) * (to - now).abs().min(1.0);
        self.from = now;
        self.to = to;
        self.ease = ease;
        self.peak = now.clamp(0.0, 1.0);
        self.t = 0.0;
        if self.secs <= 0.0 {
            self.settle();
        }
    }

    fn step(&mut self, dt: f64) -> bool {
        if self.t >= 1.0 {
            return false;
        }
        if self.secs <= 0.0 {
            self.settle();
            return false;
        }
        self.t = (self.t + dt / self.secs).min(1.0);
        if self.t >= 1.0 {
            self.settle();
            return false;
        }
        let now = self.value().clamp(0.0, 1.0);
        self.peak = if self.to >= self.from {
            self.peak.max(now)
        } else {
            self.peak.min(now)
        };
        true
    }

    fn settle(&mut self) {
        *self = Self::at(self.to);
    }

    fn value(&self) -> f64 {
        if self.t >= 1.0 {
            return self.to;
        }
        self.from + (self.to - self.from) * self.ease.map(self.t)
    }
}

#[test]
fn quick_to_matches_unit_tween_over_long_seeded_runs() {
    let eases = [
        Easing::Linear,
        Easing::OutQuad,
        Easing::InOutCubic,
        Easing::OutBack,
        Easing::OutElastic,
        Easing::OutBounce,
        Easing::InOutExp,
        Easing::Bezier {
            x1: 0.2,
            y1: 0.0,
            x2: 0.0,
            y2: 1.0,
        },
        Easing::exp_decay(0.82, 0.97, 100),
        Easing::pow(0.0, 2.0),
    ];
    for seed in 0..64u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let start = if seed % 2 == 0 { 0.0 } else { 1.0 };
        let mut o = UnitTween::at(start);
        let mut q = QuickTo::at(start);
        let mut prev_target = start;
        for step in 0..4000 {
            let r = rng.below(100);
            if r < 12 {
                // aim: a reversal back to the previous target, an end, or anywhere
                let to = match rng.below(4) {
                    0 => prev_target,
                    1 => 0.0,
                    2 => 1.0,
                    _ => rng.unit() * 1.4 - 0.2,
                };
                let secs = match rng.below(6) {
                    0 => 0.0,
                    1 => 1e-9,
                    _ => rng.unit() * 0.6,
                };
                let ease = eases[rng.below(eases.len() as u64) as usize];
                prev_target = q.target();
                o.aim(to, secs, ease);
                let moved = q.aim(to, secs, ease, Retime::ByDistance { per_unit: 1.0 });
                assert_eq!(
                    moved,
                    to != prev_target,
                    "seed {seed} step {step}: aim answer"
                );
            } else if r < 13 {
                o.settle();
                q.settle();
            } else {
                let dt = match rng.below(10) {
                    0 => 0.0,
                    1 => 1.0,
                    2 => 1e-7,
                    _ => rng.unit() / 30.0,
                };
                let a = o.step(dt);
                let b = q.step(dt);
                assert_eq!(a, b, "seed {seed} step {step}: step answer");
            }
            assert_eq!(
                o.value().to_bits(),
                q.value().to_bits(),
                "seed {seed} step {step}: value {} vs {}",
                o.value(),
                q.value()
            );
            assert_eq!(o.to.to_bits(), q.target().to_bits());
            assert_eq!(
                o.t >= 1.0,
                q.is_settled(),
                "seed {seed} step {step}: settled"
            );
        }
    }
}

/// Every hand-rolled motion QuickTo replaces is a `#[rust]` widget field or
/// lives in a `#[derive(Default)]` struct (pill_nav `PanelState.grow`,
/// line_menu `#[rust] reveal: UnitTween`), so it must be `Default`, like
/// `UnitTween` and `EasedGlide` are. Handles stored in widgets need it too.
#[test]
fn widget_field_types_are_default() {
    assert!(
        (&Probe::<TargetId>(PhantomData)).has_default(),
        "the probe itself works"
    );
    assert!(
        (&Probe::<QuickTo<f64>>(PhantomData)).has_default(),
        "QuickTo<f64> is not Default"
    );
    assert!(
        (&Probe::<QuickTo<[f64; 4]>>(PhantomData)).has_default(),
        "QuickTo<[f64; 4]> is not Default"
    );
    assert!(
        (&Probe::<TweenId>(PhantomData)).has_default(),
        "TweenId is not Default (a #[rust] handle field needs it)"
    );
    assert!(
        (&Probe::<SlotId>(PhantomData)).has_default(),
        "SlotId is not Default"
    );
}

#[test]
fn engine_is_send_sync_static() {
    fn check<T: Send + Sync + 'static>() {}
    check::<TweenEngine>();
    check::<QuickTo<f64>>();
    check::<TweenTicker>();
}

// ---------------------------------------------------------------------------
// 5. Value conversions
// ---------------------------------------------------------------------------

/// The docs promise hues in `[0, 360)`; a hue a hair below 0 must not wrap
/// to exactly 360.
#[test]
fn hues_stay_below_360() {
    let h = rgb_to_hsv([1.0, 0.0, 1e-17])[0];
    assert!((0.0..360.0).contains(&h), "rgb_to_hsv hue = {h}");
    let h = oklab_to_oklch([0.5, 0.1, -1e-18])[2];
    assert!((0.0..360.0).contains(&h), "oklab_to_oklch hue = {h}");
}

#[test]
fn rgba_u32_round_trips_and_rounds() {
    for v in 0..=255u32 {
        let packed = (v << 24) | ((255 - v) << 16) | (v.wrapping_mul(7) & 0xff) << 8 | 0x80;
        assert_eq!(Rgba::from_u32(packed).to_u32(), packed, "{packed:08x}");
    }
    assert_eq!(Rgba::new(0.5, 0.0, 1.0, 1.0).to_u32(), 0x8000_ffff);
    assert_eq!(Rgba::new(-1.0, 2.0, f64::NAN, 0.998).to_u32(), 0x00ff_00fe);
    assert_eq!(
        Rgba::new(127.49 / 255.0, 127.5 / 255.0, 0.0, 0.0).to_u32(),
        0x7f80_0000
    );
}

/// A grey end borrows the other end's OKLCH hue, so a grey -> colour tween
/// keeps that hue all the way (in the engine, not just in `TweenValue::lerp`).
#[test]
fn engine_oklch_grey_end_borrows_hue_and_alpha_is_straight() {
    let grey = Rgba::new(0.5, 0.5, 0.5, 0.0);
    let red = Rgba::new(0.9, 0.1, 0.1, 1.0);
    let red_h = oklab_to_oklch(linear_to_oklab([0.9, 0.1, 0.1].map(srgb_to_linear)))[2];
    let mut e = TweenEngine::new();
    let s = e.seed(TargetId(0), FILL, grey.into());
    e.to(
        TargetId(0).into(),
        &[PropTo::to(FILL, red.into())],
        TweenOpts::new()
            .duration(1.0)
            .ease(Easing::Linear)
            .color_space(ColorSpace::Oklch),
    );
    for k in 1..10 {
        e.advance(0.1);
        let TweenValue::Color(c) = e.value(s) else {
            panic!("not a colour")
        };
        let lch = oklab_to_oklch(linear_to_oklab([c.r, c.g, c.b].map(srgb_to_linear)));
        close(lch[2], red_h, 1e-6, &format!("hue at {k}/10"));
        close(c.a, k as f64 / 10.0, 1e-9, &format!("alpha at {k}/10"));
    }
}

/// A finished colour tween lands exactly on its target in every space
/// (GSAP writes the end value; hosts compare against theme colours).
#[test]
fn engine_colour_tween_lands_exactly_on_target_in_every_space() {
    let from = Rgba::new(0.9, 0.2, 0.1, 1.0);
    let to = Rgba::new(0.3, 0.55, 0.7, 0.75);
    for space in [
        ColorSpace::Srgb,
        ColorSpace::Linear,
        ColorSpace::Hsv,
        ColorSpace::Oklab,
        ColorSpace::Oklch,
    ] {
        let mut e = TweenEngine::new();
        let s = e.seed(TargetId(0), FILL, from.into());
        e.to(
            TargetId(0).into(),
            &[PropTo::to(FILL, to.into())],
            TweenOpts::new().duration(0.5).color_space(space),
        );
        e.advance(1.0);
        assert_eq!(
            e.value(s),
            TweenValue::Color(to),
            "{space:?}: landing value"
        );
    }
}

/// Channels stay displayable while an overshooting ease carries a colour:
/// the non-sRGB spaces clamp rgb to 0..1, so Srgb (GSAP's own space, where
/// the browser clamps) should too; alpha likewise.
#[test]
fn overshooting_colour_tween_stays_in_gamut_in_every_space() {
    let from = Rgba::new(1.0, 0.0, 0.0, 0.0);
    let to = Rgba::new(0.0, 0.0, 1.0, 1.0);
    let mut bad = Vec::new();
    for space in [
        ColorSpace::Srgb,
        ColorSpace::Linear,
        ColorSpace::Hsv,
        ColorSpace::Oklab,
        ColorSpace::Oklch,
    ] {
        let mut e = TweenEngine::new();
        let s = e.seed(TargetId(0), FILL, from.into());
        e.to(
            TargetId(0).into(),
            &[PropTo::to(FILL, to.into())],
            TweenOpts::new()
                .duration(1.0)
                .ease(Easing::OutBack)
                .color_space(space),
        );
        for k in 1..10 {
            e.advance(0.1);
            let TweenValue::Color(c) = e.value(s) else {
                panic!("not a colour")
            };
            for (name, ch) in [("r", c.r), ("g", c.g), ("b", c.b), ("a", c.a)] {
                if !(0.0..=1.0).contains(&ch) {
                    bad.push(format!("{space:?} at {k}/10: {name} = {ch}"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "out of gamut:\n{}", bad.join("\n"));
}

// ---------------------------------------------------------------------------
// 6. Easing PartialEq / Default / Debug
// ---------------------------------------------------------------------------

fn identity(t: f64) -> f64 {
    t
}

fn square(t: f64) -> f64 {
    t * t
}

#[test]
fn easing_eq_default_debug() {
    assert_eq!(Easing::default(), Easing::OutQuad);
    assert_eq!(Easing::default().map(0.5), 0.75, "power1.out(0.5)");
    let all = [
        Easing::Linear,
        Easing::Instant,
        Easing::Constant(0.5),
        Easing::InQuad,
        Easing::OutQuad,
        Easing::InOutQuad,
        Easing::InBounce,
        Easing::exp_decay(0.8, 0.9, 10),
        Easing::pow(0.0, 2.0),
        Easing::Bezier {
            x1: 0.1,
            y1: 0.2,
            x2: 0.3,
            y2: 0.4,
        },
        Easing::CubicBezier {
            x1: 0.1,
            y1: 0.2,
            x2: 0.3,
            y2: 0.4,
        },
        Easing::Back {
            dir: EaseDir::Out,
            overshoot: 1.7,
        },
        Easing::Elastic {
            dir: EaseDir::Out,
            amplitude: 1.0,
            period: 0.3,
        },
        Easing::Expo { dir: EaseDir::In },
        Easing::Expo { dir: EaseDir::Out },
        Easing::Steps {
            n: 4,
            jump: Jump::End,
        },
        Easing::GsapSteps { n: 4, start: false },
        Easing::SmoothStep,
        Easing::SmootherStep,
        Easing::Custom(CustomEase(identity)),
        Easing::Custom(CustomEase(square)),
    ];
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate() {
            assert_eq!(a == b, i == j, "{a:?} == {b:?}");
        }
        let c = *a;
        assert_eq!(&c, a, "copy equals");
        assert!(!format!("{a:?}").is_empty());
    }
    assert_ne!(Easing::Constant(0.5), Easing::Constant(0.25));
    assert_ne!(
        Easing::Back {
            dir: EaseDir::In,
            overshoot: 1.7
        },
        Easing::Back {
            dir: EaseDir::Out,
            overshoot: 1.7
        }
    );
    assert_eq!(
        YoyoEase::Ease(Easing::Custom(CustomEase(square))),
        YoyoEase::Ease(Easing::Custom(CustomEase(square)))
    );
    assert!(format!("{:?}", Easing::OutQuad).contains("OutQuad"));
    // A parsed GSAP name equals the built value (the script layer relies on it).
    assert_eq!(parse_gsap_ease("power1.out"), Some(Easing::default()));
    assert_eq!(parse_gsap_ease("power1"), Some(Easing::OutQuad));
    assert_eq!(
        parse_gsap_ease("back.out(1.7)").map(|e| e.map(1.0)),
        Some(1.0)
    );
}

// ---------------------------------------------------------------------------
// 7. Robustness of the builders
// ---------------------------------------------------------------------------

#[test]
fn degenerate_builds_do_not_panic() {
    let mut e = TweenEngine::new();
    let empty: [TargetId; 0] = [];
    e.to(
        Targets::List(&empty),
        &[PropTo::to_f64(OPACITY, 1.0)],
        TweenOpts::new(),
    );
    e.to(TargetId(0).into(), &[], TweenOpts::new());
    e.to(
        Targets::Range { first: 0, count: 0 },
        &[PropTo::to_f64(OPACITY, 1.0)],
        TweenOpts::new().stagger(Stagger::amount(1.0).from(StaggerFrom::Random(3))),
    );
    e.to(
        TargetId(0).into(),
        &[PropTo::each(OPACITY, &[])],
        TweenOpts::new(),
    );
    e.to(
        TargetId(0).into(),
        &[PropTo::values(OPACITY, &[])],
        TweenOpts::new(),
    );
    e.to(
        TargetId(0).into(),
        &[PropTo::keys(OPACITY, &[])],
        TweenOpts::new(),
    );
    e.keyframes(TargetId(0).into(), &[], TweenOpts::new());
    let tween = e.to(
        TargetId(0).into(),
        &[PropTo::to_f64(OPACITY, 1.0)],
        TweenOpts::new(),
    );
    // A tween handle is not a timeline: building into it does nothing.
    e.tl(tween).to(
        TargetId(1).into(),
        &[PropTo::to_f64(OPACITY, 1.0)],
        TweenOpts::new(),
        0.0,
    );
    assert_eq!(e.anim_ref(tween).child_count(), 0);
    // A timeline cannot contain itself or its parent.
    let outer = e.timeline(TimelineOpts::new());
    let inner = e.timeline(TimelineOpts::new());
    e.tl(outer).add(inner, 0.0);
    e.tl(outer).add(outer, 0.0);
    e.tl(inner).add(outer, 0.0);
    // Stale and NONE handles are no-ops.
    e.anim(TweenId::NONE)
        .play()
        .seek(Seek::Time(1.0), Emit::Fire);
    e.tl(TweenId::NONE).add_label(INTRO, 0.0);
    assert!(!e.anim_ref(TweenId::NONE).is_alive());
    e.anim(tween).kill();
    e.anim(tween).kill();
    e.anim(tween).restart(true, Emit::Fire);
    assert!(!e.anim_ref(tween).is_alive());
    for _ in 0..200 {
        e.advance(1.0 / 60.0);
    }
    let mut buf = Vec::new();
    e.swap_events(&mut buf);
}

// ---------------------------------------------------------------------------
// 8. The API as a widget uses it (the example lib.rs should carry)
// ---------------------------------------------------------------------------

/// A staggered entrance, a label-positioned exit, events drained after each
/// frame, values read from the slot store: what the crate-level doc example
/// should show.
#[test]
fn widget_style_usage_example() {
    const N: u32 = 5;
    const ITEMS: Targets<'static> = Targets::Range { first: 0, count: N };
    const ENTER: TweenOpts = TweenOpts::new()
        .duration(0.4)
        .stagger(Stagger::each(0.1))
        .on_complete();
    const EXIT: TweenOpts = TweenOpts::new().duration(0.3).ease(Easing::InQuad);

    let mut e = TweenEngine::new();
    for i in 0..N {
        e.seed(TargetId(i), OPACITY, 0.0.into());
        e.seed(TargetId(i), SHIFT, 20.0.into());
    }
    let tl = e.timeline(TimelineOpts::new().watch_labels().on_complete());
    e.tl(tl)
        .add_label(INTRO, 0.0)
        .to(
            ITEMS,
            &[PropTo::to_f64(OPACITY, 1.0), PropTo::to_f64(SHIFT, 0.0)],
            ENTER,
            INTRO,
        )
        .add_label(OUTRO, Position::rel(0.5))
        .to(ITEMS, &[PropTo::to_f64(OPACITY, 0.0)], EXIT, OUTRO);

    let r = e.anim_ref(tl);
    close(r.duration(), 0.8 + 0.5 + 0.3, 1e-12, "timeline duration");
    close(r.label_time(OUTRO).unwrap(), 1.3, 1e-12, "outro label");
    assert_eq!(r.child_count(), 2);

    let mut events = Vec::new();
    let mut seen = Vec::new();
    let frame = 1.0 / 64.0; // exact in binary, so frame sums are exact
    fn run(e: &mut TweenEngine, frames: u32, frame: f64, seen: &mut Vec<TweenEvent>) {
        let mut events = Vec::new();
        for _ in 0..frames {
            e.advance(frame);
            e.swap_events(&mut events);
            seen.append(&mut events);
            e.clear_changes();
        }
    }

    // 0.25 s in: item 0 at 0.625 of its run, item 1 at 0.375, item 2 at 0.125.
    run(&mut e, 16, frame, &mut seen);
    let out = |p: f64| 1.0 - (1.0 - p) * (1.0 - p); // power1.out
    for (i, p) in [(0u32, 0.625), (1, 0.375), (2, 0.125), (3, 0.0), (4, 0.0)] {
        close(
            e.get_f64(TargetId(i), OPACITY).unwrap(),
            out(p),
            1e-12,
            &format!("opacity of item {i} at 0.25 s"),
        );
        close(
            e.get_f64(TargetId(i), SHIFT).unwrap(),
            20.0 * (1.0 - out(p)),
            1e-12,
            &format!("shift of item {i} at 0.25 s"),
        );
    }
    // One more frame: only moving slots are reported as changed.
    e.advance(frame);
    let changed: Vec<TargetId> = e.changes().iter().map(|&s| e.slot_key(s).0).collect();
    assert!(changed.contains(&TargetId(0)) && changed.contains(&TargetId(2)));
    assert!(
        !changed.contains(&TargetId(4)),
        "item 4 has not started: {changed:?}"
    );
    e.swap_events(&mut events);
    seen.extend(events.drain(..));
    e.clear_changes();

    // Past the end.
    run(&mut e, 128, frame, &mut seen);
    for i in 0..N {
        assert_eq!(e.get_f64(TargetId(i), OPACITY), Some(0.0));
        assert_eq!(e.get_f64(TargetId(i), SHIFT), Some(0.0));
    }
    let kinds: Vec<(TweenId, EventKind)> = seen.iter().map(|ev| (ev.id, ev.kind)).collect();
    let outro_at = kinds
        .iter()
        .position(|k| *k == (tl, EventKind::Label(OUTRO)))
        .expect("the outro label is reported");
    let done_at = kinds
        .iter()
        .position(|k| *k == (tl, EventKind::Complete))
        .expect("the timeline completes");
    assert!(outro_at < done_at);
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == (tl, EventKind::Complete))
            .count(),
        1
    );
    assert!(!e.is_active(), "nothing left to animate");
    assert!(e.anim_ref(tl).is_alive(), "timelines are kept by default");

    // A kept timeline replays.
    e.anim(tl).restart(false, Emit::Suppress);
    assert!(e.is_active());
    run(&mut e, 16, frame, &mut seen);
    close(
        e.get_f64(TargetId(0), OPACITY).unwrap(),
        out(0.625),
        1e-12,
        "replayed item 0",
    );
}

// ---------------------------------------------------------------------------
// 9. Slot store and sample() contracts
// ---------------------------------------------------------------------------

/// Spec 2.8: `seed` "marks changed". A host seeds on creation and pushes
/// `changes()`; a seed that happens to equal the slot's initial lanes (0.0)
/// or its current value must still be listed.
#[test]
fn seed_always_marks_the_slot_changed() {
    let mut e = TweenEngine::new();
    let s = e.seed(TargetId(0), OPACITY, 0.0.into());
    assert!(
        e.is_changed(s),
        "a fresh slot seeded with 0.0 is not in changes()"
    );
    assert_eq!(e.changes(), &[s]);
    e.clear_changes();
    e.seed(TargetId(0), OPACITY, 0.0.into());
    assert!(
        e.is_changed(s),
        "re-seeding the same value is not in changes()"
    );
}

/// `sample` is documented as the value the tween gives the slot at its own
/// total time; it must agree with what the render wrote, for every kind and
/// post-step (Int rounding, snap, colour decode, yoyo).
#[test]
fn sample_agrees_with_the_rendered_value() {
    let mut e = TweenEngine::new();
    let a = e.seed(TargetId(0), OPACITY, 0.0.into());
    let b = e.seed(TargetId(0), SHIFT, TweenValue::Int(-7));
    let c = e.seed(TargetId(0), FILL, Rgba::new(0.9, 0.1, 0.1, 1.0).into());
    let d = e.seed(TargetId(0), PropKey(4), [0.0, 10.0].into());
    let id = e.to(
        TargetId(0).into(),
        &[
            PropTo::to_f64(OPACITY, 1.0),
            PropTo::to(SHIFT, TweenValue::Int(12)),
            PropTo::to(FILL, Rgba::new(0.1, 0.3, 0.9, 0.5).into()),
            PropTo::to(PropKey(4), [5.0, -5.0].into()).snap(0.25),
        ],
        TweenOpts::new()
            .duration(0.7)
            .ease(Easing::OutBack)
            .repeat(2)
            .yoyo(true)
            .yoyo_ease(YoyoEase::Ease(Easing::InCubic))
            .color_space(ColorSpace::Oklch),
    );
    for k in 0..130 {
        e.advance(1.0 / 60.0);
        let tt = e.anim_ref(id).total_time();
        if !e.anim_ref(id).is_alive() {
            break;
        }
        for s in [a, b, c, d] {
            assert_eq!(
                e.sample(id, s, tt),
                Some(e.value(s)),
                "frame {k}, total time {tt}, slot {s:?}"
            );
        }
    }
}

#[test]
fn get_by_tag_and_tag_round_trip() {
    let mut e = TweenEngine::new();
    e.seed(TargetId(0), OPACITY, 0.0.into());
    let t = e.to(
        TargetId(0).into(),
        &[PropTo::to_f64(OPACITY, 1.0)],
        TweenOpts::new().tag(INTRO),
    );
    let tl = e.timeline(TimelineOpts::new().tag(OUTRO));
    assert_eq!(e.get_by_tag(INTRO), Some(t));
    assert_eq!(e.get_by_tag(OUTRO), Some(tl));
    assert_eq!(e.get_by_tag(Tag::NONE), None);
    assert_eq!(e.anim_ref(t).tag(), INTRO);
    assert_eq!(e.anim_ref(tl).tag(), OUTRO);
}

// ---------------------------------------------------------------------------
// 10. Allocation contracts of the leaf API
// ---------------------------------------------------------------------------

/// Spec 2.8: "new() allocates nothing (empty Vecs)". Widgets hold an engine
/// in a `#[rust]` field, built with `Default` for every widget instance.
#[test]
fn engine_new_and_default_allocate_nothing() {
    let (n, e) = allocations(TweenEngine::new);
    drop(e);
    assert_eq!(n, 0, "TweenEngine::new() allocated {n} times");
    let (n, e) = allocations(TweenEngine::default);
    drop(e);
    assert_eq!(n, 0, "TweenEngine::default() allocated {n} times");
}

#[test]
fn leaf_helpers_allocate_nothing() {
    let s = Stagger::each(0.1)
        .from(StaggerFrom::Random(5))
        .grid(3, 3)
        .ease(Easing::InQuad);
    let mut out = [0.0; 9];
    let (n, _) = allocations(|| {
        s.delays(9, &mut out);
        let mut acc = 0.0;
        for i in 0..9 {
            acc += s.delay(i, 9);
        }
        let p = Position::parse("intro+=50%", label_of);
        let q = Position::parse("<+=0.25", label_of);
        let ease = parse_gsap_ease("elastic.inOut(1.2, 0.4)");
        let mut quick = QuickTo::at([0.0f64; 4]);
        quick.aim([1.0; 4], 0.3, Easing::OutBack, Retime::Full);
        quick.step(0.1);
        (acc, p, q, ease, quick.value())
    });
    assert_eq!(n, 0);
}

/// Host-chosen target ids near `u32::MAX` must not overflow in
/// `Targets::Range` (`first + i` panics in debug builds).
#[test]
fn target_range_near_u32_max_does_not_overflow() {
    let r = Targets::Range {
        first: u32::MAX - 1,
        count: 3,
    };
    let got = std::panic::catch_unwind(|| (0..r.len()).map(|i| r.get(i)).collect::<Vec<_>>());
    assert!(got.is_ok(), "Targets::Range::get overflowed");
}
