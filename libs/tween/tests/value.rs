//! Values: lanes, integer rounding, colour spaces and colour interpolation.

mod common;

use common::Rng;
use makepad_tween::{
    decode, encode, encode_pair, linear_to_oklab, oklab_to_oklch, srgb_to_linear, ColorSpace, Rgba,
    TweenValue, ValueKind,
};

const RED: Rgba = Rgba::new(1.0, 0.0, 0.0, 1.0);
const BLUE: Rgba = Rgba::new(0.0, 0.0, 1.0, 1.0);

fn oklab(c: Rgba) -> [f64; 3] {
    linear_to_oklab([c.r, c.g, c.b].map(srgb_to_linear))
}

fn close3(got: [f64; 3], want: [f64; 3], tol: f64, what: &str) {
    for i in 0..3 {
        assert!(
            (got[i] - want[i]).abs() <= tol,
            "{what}[{i}]: got {got:?}, want {want:?}"
        );
    }
}

fn mid(a: Rgba, b: Rgba, space: ColorSpace) -> Rgba {
    match TweenValue::lerp(&a.into(), &b.into(), 0.5, space) {
        TweenValue::Color(c) => c,
        other => panic!("{other:?}"),
    }
}

#[test]
fn oklab_golden() {
    close3(
        oklab(RED),
        [0.6279553606, 0.2248630611, 0.1258462985],
        1e-9,
        "red",
    );
    close3(
        oklab(BLUE),
        [0.4520137184, -0.0324569842, -0.3115281477],
        1e-9,
        "blue",
    );
    close3(
        oklab(Rgba::new(1.0, 1.0, 1.0, 1.0)),
        [1.0, 0.0, 0.0],
        1e-7,
        "white",
    );
}

#[test]
fn oklch_golden() {
    close3(
        oklab_to_oklch(oklab(RED)),
        [0.6279553606, 0.2576833077, 29.2338851923],
        1e-8,
        "red",
    );
}

#[test]
fn every_space_round_trips_within_1e9() {
    let mut rng = Rng(0x5eed);
    let spaces = [
        ColorSpace::Srgb,
        ColorSpace::Linear,
        ColorSpace::Hsv,
        ColorSpace::Oklab,
        ColorSpace::Oklch,
    ];
    for _ in 0..1000 {
        let c = Rgba::new(
            rng.next_f64(),
            rng.next_f64(),
            rng.next_f64(),
            rng.next_f64(),
        );
        for space in spaces {
            let back = decode(encode(c, space), space);
            let (got, want) = ([back.r, back.g, back.b, back.a], [c.r, c.g, c.b, c.a]);
            for i in 0..4 {
                assert!(
                    (got[i] - want[i]).abs() < 1e-9,
                    "{space:?}: {c:?} -> {back:?}"
                );
            }
        }
    }
}

#[test]
fn hue_takes_the_short_way() {
    assert_eq!(mid(RED, BLUE, ColorSpace::Hsv).to_u32(), 0xff00ffff);
    let (a, b) = encode_pair(RED, BLUE, ColorSpace::Oklch);
    assert!(
        (b[2] - a[2] - -125.1818645543).abs() < 1e-8,
        "delta {}",
        b[2] - a[2]
    );
    // Both directions take the short way.
    let (a, b) = encode_pair(BLUE, RED, ColorSpace::Oklch);
    assert!(
        (b[2] - a[2] - 125.1818645543).abs() < 1e-8,
        "delta {}",
        b[2] - a[2]
    );
}

#[test]
fn achromatic_end_borrows_the_other_hue() {
    let grey = Rgba::new(0.5, 0.5, 0.5, 1.0);
    let (a, b) = encode_pair(grey, RED, ColorSpace::Oklch);
    let red_hue = encode(RED, ColorSpace::Oklch)[2];
    assert_eq!(a[2], red_hue);
    assert_eq!(b[2], red_hue);
    for k in 0..=10 {
        let r = k as f64 / 10.0;
        let lanes = makepad_tween::lerp_lanes(a, b, r);
        assert_eq!(lanes[2], red_hue, "hue at {r}");
    }
    // Two greys: hue 0 on both ends.
    let (a, b) = encode_pair(grey, Rgba::new(0.1, 0.1, 0.1, 1.0), ColorSpace::Hsv);
    assert_eq!((a[0], b[0]), (0.0, 0.0));
}

#[test]
fn midpoints_red_to_blue() {
    assert_eq!(mid(RED, BLUE, ColorSpace::Srgb).to_u32(), 0x800080ff);
    assert_eq!(mid(RED, BLUE, ColorSpace::Linear).to_u32(), 0xbc00bcff);
    assert_eq!(mid(RED, BLUE, ColorSpace::Oklab).to_u32(), 0x8c53a2ff);
}

#[test]
fn rgba_to_u32_rounds() {
    let r = |c: f64| Rgba::new(c, 0.0, 0.0, 0.0).to_u32() >> 24;
    assert_eq!(r(0.5), 0x80);
    assert_eq!(r(0.998), 254);
    assert_eq!(r(1.0 / 255.0), 1);
    assert_eq!(r(254.5 / 255.0), 255);
    assert_eq!(r(-0.5), 0);
    assert_eq!(r(1.5), 255);
    assert_eq!(Rgba::from_u32(0x11223344).to_u32(), 0x11223344);
    assert_eq!(
        Rgba::from_f32([1.0, 0.5, 0.25, 1.0]).to_f32(),
        [1.0, 0.5, 0.25, 1.0]
    );
}

#[test]
fn int_rounds_half_away_from_zero() {
    let at_half = |b: i64| {
        TweenValue::lerp(
            &TweenValue::Int(0),
            &TweenValue::Int(b),
            0.5,
            ColorSpace::Srgb,
        )
    };
    assert_eq!(at_half(5), TweenValue::Int(3));
    assert_eq!(at_half(-5), TweenValue::Int(-3));
    assert_eq!(
        TweenValue::from_lanes(ValueKind::Int, [2.5, 0.0, 0.0, 0.0]),
        TweenValue::Int(3)
    );
    assert_eq!(
        TweenValue::from_lanes(ValueKind::Int, [-2.5, 0.0, 0.0, 0.0]),
        TweenValue::Int(-3)
    );
}

#[test]
fn linear_midpoint_is_brighter_than_srgb() {
    let black = Rgba::new(0.0, 0.0, 0.0, 1.0);
    let white = Rgba::new(1.0, 1.0, 1.0, 1.0);
    let s = mid(black, white, ColorSpace::Srgb);
    let l = mid(black, white, ColorSpace::Linear);
    assert!(l.r > s.r, "linear {} vs srgb {}", l.r, s.r);
    assert!((s.r - 0.5).abs() < 1e-15);
}

#[test]
fn lanes_round_trip_every_kind() {
    let values = [
        TweenValue::F64(1.5),
        TweenValue::Vec2([1.0, 2.0]),
        TweenValue::Vec3([1.0, 2.0, 3.0]),
        TweenValue::Vec4([1.0, 2.0, 3.0, 4.0]),
        TweenValue::Int(-7),
        TweenValue::Color(Rgba::new(0.1, 0.2, 0.3, 0.4)),
    ];
    for v in values {
        assert_eq!(TweenValue::from_lanes(v.kind(), v.to_lanes()), v);
    }
    assert_eq!(
        TweenValue::Vec2([3.0, 4.0]).to_lanes(),
        [3.0, 4.0, 0.0, 0.0]
    );
    assert_eq!(TweenValue::Int(-7).as_f64(), -7.0);
    assert_eq!(TweenValue::from(2.0), TweenValue::F64(2.0));
    assert_eq!(TweenValue::from([1.0, 2.0]).kind(), ValueKind::Vec2);
    assert_eq!(TweenValue::from([1.0, 2.0, 3.0]).kind(), ValueKind::Vec3);
    assert_eq!(
        TweenValue::from([1.0, 2.0, 3.0, 4.0]).kind(),
        ValueKind::Vec4
    );
    assert_eq!(TweenValue::from(3i64), TweenValue::Int(3));
    assert_eq!(TweenValue::from(RED).kind(), ValueKind::Color);
    let v = TweenValue::lerp(
        &TweenValue::Vec2([0.0, 10.0]),
        &TweenValue::Vec2([10.0, 20.0]),
        0.25,
        ColorSpace::Srgb,
    );
    assert_eq!(v, TweenValue::Vec2([2.5, 12.5]));
}

#[test]
fn decode_clamps_out_of_gamut_and_keeps_alpha() {
    let c = decode([0.9, 0.5, 0.5, 0.25], ColorSpace::Oklab);
    for ch in [c.r, c.g, c.b] {
        assert!((0.0..=1.0).contains(&ch), "{c:?}");
    }
    assert_eq!(c.a, 0.25);
    let hsv = decode([-60.0, 1.0, 1.0, 1.0], ColorSpace::Hsv);
    assert_eq!(hsv.to_u32(), 0xff00ffff);
}
