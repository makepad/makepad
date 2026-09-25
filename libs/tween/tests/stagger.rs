//! Stagger: GSAP `utils.distribute` goldens and the delay / delays contract.

#[path = "golden/stagger.rs"]
mod golden_stagger;

use golden_stagger::{Case, DISTRIBUTE, IN_TWEEN};
use makepad_tween::{parse_gsap_ease, Easing, Spread, Stagger, StaggerAxis, StaggerFrom};

fn delays(s: Stagger, n: u32) -> Vec<f64> {
    let mut out = vec![0.0; n as usize];
    s.delays(n, &mut out);
    out
}

fn assert_delays(got: &[f64], want: &[f64], tol: f64, what: &str) {
    assert_eq!(got.len(), want.len(), "{what}");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() <= tol,
            "{what}[{i}]: got {got:?}, want {want:?}"
        );
    }
}

fn golden_stagger(case: &Case) -> Stagger {
    let mut s = case.stagger;
    if let Some(name) = case.ease {
        s.ease = Some(parse_gsap_ease(name).unwrap_or_else(|| panic!("{}: ease {name}", case.id)));
    }
    s
}

#[test]
fn every_gsap_distribute_case() {
    for case in &DISTRIBUTE {
        let s = golden_stagger(case);
        let what = format!("{} {}", case.id, case.config);
        let got = delays(s, case.n);
        assert_delays(&got, case.delays, 1e-12, &what);
        for i in 0..case.n {
            assert_eq!(
                s.delay(i, case.n).to_bits(),
                got[i as usize].to_bits(),
                "{what} delay({i})"
            );
        }
    }
}

#[test]
fn every_gsap_staggered_tween_case() {
    // Sub-tween start times of `gsap.to(objs, {duration: 1, stagger})`, and the
    // tween's duration: the largest delay plus the duration.
    for case in &IN_TWEEN {
        let s = golden_stagger(case);
        let what = format!("{} {}", case.id, case.config);
        let got = delays(s, case.n);
        assert_delays(&got, case.delays, 1e-12, &what);
        let longest = got.iter().fold(0.0f64, |m, d| m.max(*d)) + 1.0;
        assert!(
            (longest - case.tween_duration).abs() <= 1e-12,
            "{what}: duration {longest}"
        );
    }
}

#[test]
fn one_dimensional_golden() {
    let each = Stagger::each(0.1);
    let tol = 1e-12;
    assert_delays(&delays(each, 5), &[0.0, 0.1, 0.2, 0.3, 0.4], tol, "index 0");
    assert_delays(
        &delays(each.from(StaggerFrom::Start), 5),
        &[0.0, 0.1, 0.2, 0.3, 0.4],
        tol,
        "start",
    );
    assert_delays(
        &delays(each.from(StaggerFrom::End), 5),
        &[0.4, 0.3, 0.2, 0.1, 0.0],
        tol,
        "end",
    );
    assert_delays(
        &delays(each.from(StaggerFrom::Center), 5),
        &[0.4, 0.2, 0.0, 0.2, 0.4],
        tol,
        "center",
    );
    assert_delays(
        &delays(each.from(StaggerFrom::Edges), 5),
        &[0.0, 0.2, 0.4, 0.2, 0.0],
        tol,
        "edges",
    );
    assert_delays(
        &delays(each.from(StaggerFrom::Index(1)), 5),
        &[0.1333333, 0.0, 0.1333333, 0.2666667, 0.4],
        tol,
        "index 1",
    );
    assert_delays(
        &delays(each.from(StaggerFrom::Ratio(0.25, 0.25)), 5),
        &[0.0666667, 0.0, 0.1333333, 0.2666667, 0.4],
        tol,
        "ratio .25",
    );
    assert_delays(
        &delays(Stagger::amount(1.0), 5),
        &[0.0, 0.25, 0.5, 0.75, 1.0],
        tol,
        "amount 1",
    );
    assert_delays(
        &delays(each.from(StaggerFrom::Center), 4),
        &[0.3, 0.0, 0.0, 0.3],
        tol,
        "center n4",
    );
    assert_delays(
        &delays(each.from(StaggerFrom::Edges), 4),
        &[0.0, 0.3, 0.3, 0.0],
        tol,
        "edges n4",
    );
}

#[test]
fn grid_golden() {
    let tol = 1e-12;
    let g = Stagger::each(0.05).grid(3, 4);
    #[rustfmt::skip]
    let center = [
        0.2, 0.0948796, 0.0948796, 0.2,
        0.1535184, 0.0, 0.0, 0.1535184,
        0.2, 0.0948796, 0.0948796, 0.2,
    ];
    assert_delays(
        &delays(g.from(StaggerFrom::Center), 12),
        &center,
        tol,
        "center",
    );
    let x = delays(g.from(StaggerFrom::Center).axis(StaggerAxis::X), 12);
    for row in x.chunks(4) {
        assert_delays(row, &[0.2, 0.0, 0.0, 0.2], tol, "axis x");
    }
    #[rustfmt::skip]
    let y = [0.15, 0.15, 0.15, 0.15, 0.0, 0.0, 0.0, 0.0, 0.15, 0.15, 0.15, 0.15];
    assert_delays(
        &delays(g.from(StaggerFrom::Center).axis(StaggerAxis::Y), 12),
        &y,
        tol,
        "axis y",
    );
    #[rustfmt::skip]
    let start = [
        0.0, 0.0486376, 0.102525, 0.1573947,
        0.0486376, 0.0786974, 0.1228901, 0.1725504,
        0.102525, 0.1228901, 0.1573947, 0.2,
    ];
    assert_delays(
        &delays(g.from(StaggerFrom::Start), 12),
        &start,
        tol,
        "start",
    );
    #[rustfmt::skip]
    let index0 = [
        0.0, 0.05547, 0.11094, 0.1664101,
        0.05547, 0.0784465, 0.1240347, 0.1754116,
        0.11094, 0.1240347, 0.1568929, 0.2,
    ];
    assert_delays(&delays(g, 12), &index0, tol, "index 0");

    let big = delays(Stagger::each(0.04).grid(5, 8).from(StaggerFrom::Center), 40);
    for corner in [0, 7, 32, 39] {
        assert!(
            (big[corner] - 0.32).abs() <= tol,
            "corner {corner}: {}",
            big[corner]
        );
    }
    #[rustfmt::skip]
    let row2 = [0.2718677, 0.1812452, 0.0906226, 0.0, 0.0, 0.0906226, 0.1812452, 0.2718677];
    assert_delays(&big[16..24], &row2, tol, "5x8 row 2");
}

#[test]
fn eased_golden() {
    let tol = 1e-12;
    let s = Stagger::each(0.1).ease(Easing::InQuad);
    assert_delays(
        &delays(s.from(StaggerFrom::Center), 5),
        &[0.4, 0.1, 0.0, 0.1, 0.4],
        tol,
        "center",
    );
    // Edges negates the span and inverts the ease (GSAP _invertEase).
    assert_delays(
        &delays(s.from(StaggerFrom::Edges), 5),
        &[0.0, 0.1, 0.4, 0.1, 0.0],
        tol,
        "edges",
    );
}

#[test]
fn random_is_a_seeded_permutation() {
    let r = |seed| delays(Stagger::each(0.1).from(StaggerFrom::Random(seed)), 12);
    assert_eq!(r(7), r(7));
    assert_ne!(r(7), r(8));
    let mut shuffled = r(7);
    let mut plain = delays(Stagger::each(0.1).from(StaggerFrom::Start), 12);
    shuffled.sort_by(f64::total_cmp);
    plain.sort_by(f64::total_cmp);
    assert_delays(&shuffled, &plain, 1e-12, "same multiset");
    assert_ne!(
        r(7),
        delays(Stagger::each(0.1).from(StaggerFrom::Start), 12),
        "actually shuffled"
    );
}

#[test]
fn delay_equals_delays() {
    let forms = [
        Stagger::each(0.1),
        Stagger::each(0.1).from(StaggerFrom::End),
        Stagger::each(0.1).from(StaggerFrom::Center),
        Stagger::each(0.1).from(StaggerFrom::Edges),
        Stagger::each(0.1).from(StaggerFrom::Index(3)),
        Stagger::each(0.1).from(StaggerFrom::Ratio(0.3, 0.7)),
        Stagger::each(0.1).from(StaggerFrom::Random(42)),
        Stagger::amount(2.0)
            .from(StaggerFrom::Center)
            .ease(Easing::OutQuad),
        Stagger::each(0.05).grid(3, 4).from(StaggerFrom::Center),
        Stagger::each(0.05)
            .grid(4, 3)
            .axis(StaggerAxis::Y)
            .from(StaggerFrom::Random(9)),
        Stagger::each_within(0.03, 0.15)
            .from(StaggerFrom::Edges)
            .base(0.5),
        Stagger::each(0.1).ease(Easing::InBack).base(0.25),
    ];
    for s in forms {
        for n in [1u32, 2, 5, 12, 40, 300] {
            let all = delays(s, n);
            for i in 0..n {
                assert_eq!(
                    s.delay(i, n).to_bits(),
                    all[i as usize].to_bits(),
                    "{s:?} n {n} i {i}"
                );
            }
        }
    }
}

#[test]
fn each_within_is_the_floating_action_budget() {
    for n in 1..=48u32 {
        let s = Stagger::each_within(0.03, 0.15);
        let want = if n >= 2 {
            0.03f64.min(0.15 / (n - 1) as f64)
        } else {
            0.03
        };
        assert_eq!(s.step(n).to_bits(), want.to_bits(), "n {n}");
        assert_eq!(
            s.spread,
            Spread::EachWithin {
                each: 0.03,
                amount: 0.15
            }
        );
    }
}

#[test]
fn fast_path_is_order_times_step() {
    for s in [0.03, 0.1, 1.0 / 3.0] {
        for n in [1u32, 2, 7, 33] {
            for i in 0..n {
                let fwd = Stagger::each(s).delay(i, n);
                assert_eq!(fwd.to_bits(), (i as f64 * s).to_bits());
                let start = Stagger::each(s).from(StaggerFrom::Start).delay(i, n);
                assert_eq!(start.to_bits(), (i as f64 * s).to_bits());
                let back = Stagger::each(s).from(StaggerFrom::End).delay(i, n);
                assert_eq!(back.to_bits(), ((n - 1 - i) as f64 * s).to_bits());
            }
        }
    }
}

#[test]
fn builders_and_step() {
    let s = Stagger::amount(1.0)
        .grid(2, 3)
        .axis(StaggerAxis::X)
        .base(0.5)
        .each_repeat(2)
        .each_yoyo(true)
        .each_repeat_delay(0.25);
    assert_eq!(s.repeat, Some(2));
    assert_eq!(s.yoyo, Some(true));
    assert_eq!(s.repeat_delay, Some(0.25));
    assert_eq!(s.base, 0.5);
    assert_eq!(s.step(5), 0.25);
    assert_eq!(s.step(1), 0.0);
    assert_eq!(Stagger::each(0.2).step(9), 0.2);
    let mut none: [f64; 0] = [];
    s.delays(0, &mut none);
}
