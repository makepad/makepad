//! Eases: bit parity with Makepad's `Ease::map`, GSAP 3.15 goldens, CSS forms.

mod common;
#[path = "golden/eases.rs"]
mod golden_eases;

use common::{to_easing, Ease, PLAIN_ARMS, THEME_EASES};
use golden_eases::{BEZIERS, EASES, T};
use makepad_tween::{parse_gsap_ease, CustomEase, EaseDir, Easing, Jump, CSS_PRESETS};

/// t = k / 4096 for k in -512..=4608: the unit range plus 1/8 either side.
fn parity_grid() -> impl Iterator<Item = f64> {
    (-512..=4608).map(|k| k as f64 / 4096.0)
}

fn assert_bit_parity(oracle: &Ease, ours: &Easing) {
    for t in parity_grid() {
        let want = oracle.map(t);
        let got = ours.map(t);
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "{oracle:?} vs {ours:?} at t = {t}: {got} != {want}"
        );
    }
}

fn close(got: f64, want: f64, tol: f64, what: &str) {
    assert!(
        (got - want).abs() <= tol,
        "{what}: got {got}, want {want} (tol {tol})"
    );
}

#[test]
fn parity_family_is_bit_identical_to_ease_map() {
    let mut cases: Vec<Ease> = PLAIN_ARMS.to_vec();
    cases.extend([
        Ease::Constant(0.3),
        Ease::Constant(-1.0),
        Ease::Constant(2.0),
        Ease::ExpDecay {
            d1: 0.82,
            d2: 0.97,
            max: 100,
        },
        Ease::ExpDecay {
            d1: 0.80,
            d2: 0.97,
            max: 100,
        },
        Ease::ExpDecay {
            d1: 0.82,
            d2: 0.95,
            max: 100,
        },
        Ease::ExpDecay {
            d1: 0.82,
            d2: 0.97,
            max: 5,
        },
        Ease::Pow {
            begin: 0.0,
            end: 1.0,
        },
        Ease::Pow {
            begin: 2.0,
            end: 3.0,
        },
    ]);
    cases.extend(THEME_EASES);
    for oracle in &cases {
        assert_bit_parity(oracle, &to_easing(oracle));
    }
    for (name, [x1, y1, x2, y2]) in CSS_PRESETS {
        let oracle = Ease::Bezier {
            cp0: x1,
            cp1: y1,
            cp2: x2,
            cp3: y2,
        };
        let ours = Easing::css_preset(name).expect(name);
        assert_eq!(ours, to_easing(&oracle));
        assert_bit_parity(&oracle, &ours);
    }
}

#[test]
fn exp_decay_step_counts_are_17_16_14() {
    let steps = |d1, d2| match Easing::exp_decay(d1, d2, 100) {
        Easing::ExpDecay { steps, max, .. } => {
            assert_eq!(max, 100);
            steps
        }
        other => panic!("{other:?}"),
    };
    assert_eq!(steps(0.82, 0.97), 17);
    assert_eq!(steps(0.80, 0.97), 16);
    assert_eq!(steps(0.82, 0.95), 14);
    match Easing::exp_decay(0.82, 0.97, 5000) {
        Easing::ExpDecay { max, .. } => assert_eq!(max, 1000),
        other => panic!("{other:?}"),
    }
}

#[test]
fn gsap_family_golden() {
    let back = |dir, s| Easing::Back { dir, overshoot: s };
    let elastic = |dir, a, p| Easing::Elastic {
        dir,
        amplitude: a,
        period: p,
    };
    let expo = |dir| Easing::Expo { dir };
    let tol = 1e-12;
    close(
        back(EaseDir::Out, 1.7).map(0.5),
        1.0875,
        tol,
        "back.out(1.7)(.5)",
    );
    close(
        back(EaseDir::InOut, 1.7).map(0.25),
        -0.04375,
        tol,
        "back.inOut(1.7)(.25)",
    );
    close(
        elastic(EaseDir::Out, 1.0, 0.3).map(0.5),
        1.015625,
        tol,
        "elastic.out(1,.3)(.5)",
    );
    close(
        elastic(EaseDir::InOut, 1.0, 0.45).map(0.25),
        0.011969444423734,
        tol,
        "elastic.inOut(1,.45)(.25)",
    );
    close(
        elastic(EaseDir::In, 1.0, 0.3).map(0.9),
        -0.25,
        tol,
        "elastic.in(1,.3)(.9)",
    );
    // GSAP 3's expo is the blended curve 2^(10(p-1))·p + p⁶·(1-p) [golden: expo_in].
    close(expo(EaseDir::In).map(0.5), 0.0234375, tol, "expo.in(.5)");
    close(expo(EaseDir::Out).map(0.5), 0.9765625, tol, "expo.out(.5)");
    assert_eq!(expo(EaseDir::In).map(0.0), 0.0);
    assert_eq!(expo(EaseDir::Out).map(1.0), 1.0);
    assert_eq!(expo(EaseDir::InOut).map(0.0), 0.0);
    assert_eq!(expo(EaseDir::InOut).map(1.0), 1.0);
    // Overshooting eases leave 0..1 [golden: back_out, elastic_out].
    close(
        back(EaseDir::Out, 1.70158).map(0.6),
        1.09935168,
        1e-9,
        "back.out(.6)",
    );
    close(
        elastic(EaseDir::Out, 1.0, 0.3).map(0.1),
        1.25,
        tol,
        "elastic.out(.1)",
    );
}

#[test]
fn gsap_eases_match_gsap_3_15() {
    for (id, name, want) in EASES {
        let ease =
            parse_gsap_ease(name).unwrap_or_else(|| panic!("{id}: parse_gsap_ease({name:?})"));
        for (t, w) in T.iter().zip(want) {
            close(ease.map(*t), w, 1e-12, &format!("{id} ({name}) at {t}"));
        }
    }
}

#[test]
fn gsap_steps_match_the_golden_tables() {
    // GSAP steps(n) is SteppedEase: floor((n+1)·clamp(p, 0, 1-1e-8)) · (1/n), bit for bit
    // (the CSS Steps{n+1, None} has the same levels but divides: 3/5 = 0.6 where
    // GSAP gives 3·(1/5) = 0.6000000000000001) [golden: steps_1, steps_3, steps_5, steps_12].
    for n in [1u32, 3, 5, 12] {
        let name = format!("steps({n})");
        let ease = parse_gsap_ease(&name).unwrap();
        assert_eq!(ease, Easing::GsapSteps { n, start: false });
        let id = format!("steps_{n}");
        let (_, _, want) = EASES.iter().find(|(i, _, _)| *i == id).expect(&id);
        for (t, w) in T.iter().zip(want) {
            assert_eq!(ease.map(*t).to_bits(), w.to_bits(), "{name} at {t}");
        }
    }
}

#[test]
fn steps_css_forms() {
    let s = |jump| Easing::Steps { n: 4, jump };
    assert_eq!(s(Jump::End).map(0.5), 0.5);
    assert_eq!(s(Jump::Start).map(0.5), 0.75);
    assert_eq!(s(Jump::Both).map(0.5), 0.6);
    assert_eq!(s(Jump::None).map(0.5), 0.6666666666666666);
    assert_eq!(s(Jump::Start).map(0.0), 0.25);
    assert_eq!(s(Jump::Both).map(0.0), 0.2);
    assert_eq!(s(Jump::End).map(0.0), 0.0);
    assert_eq!(s(Jump::None).map(0.0), 0.0);
    for jump in [Jump::Start, Jump::End, Jump::Both, Jump::None] {
        assert_eq!(s(jump).map(1.0), 1.0, "{jump:?}");
        assert_eq!(s(jump).map(1.5), 1.0, "{jump:?}");
        assert_eq!(s(jump).map(-0.5), 0.0, "{jump:?}");
    }
}

#[test]
fn gsap_steps_parse() {
    let e = parse_gsap_ease("steps(5)").unwrap();
    assert_eq!(e, Easing::GsapSteps { n: 5, start: false });
    assert_eq!(e.map(0.1), 0.0);
    assert_eq!(e.map(0.5).to_bits(), 0.6000000000000001f64.to_bits());
    assert_eq!(e.map(0.9), 1.0);
    assert_eq!(e.map(1.0), 1.0);
    // The CSS form with the same levels differs in the last bit [golden: steps_5].
    let css = Easing::Steps {
        n: 6,
        jump: Jump::None,
    };
    assert_eq!(css.map(0.5), 0.6);
    let s = parse_gsap_ease("steps(5, true)").unwrap();
    assert_eq!(s, Easing::GsapSteps { n: 5, start: true });
    // [golden: steps_5_true] ((floor(5 * .5) + 1) * (1/5)).
    assert_eq!(s.map(0.5).to_bits(), 0.6000000000000001f64.to_bits());
    assert_eq!(s.map(0.0).to_bits(), 0.2f64.to_bits());
    assert_eq!(s.map(1.0), 1.0);
    assert_eq!(parse_gsap_ease("steps(0)"), None);
    assert_eq!(parse_gsap_ease("steps"), None);
}

#[test]
fn precise_bezier_golden() {
    // Reference values from a 50-digit bisection of the exact curve (the
    // spec's recalled 0.4085105930 and 0.6072203638 were 1.6e-9 and 1.7e-9 off).
    let tol = 1e-12;
    close(
        Easing::css(0.25, 0.1, 0.25, 1.0).map(0.5),
        0.802403387584857,
        tol,
        "ease(.5)",
    );
    close(
        Easing::css(0.25, 0.1, 0.25, 1.0).map(0.25),
        0.408510591355396,
        tol,
        "ease(.25)",
    );
    close(
        Easing::css(0.42, 0.0, 0.58, 1.0).map(0.5),
        0.5,
        tol,
        "ease-in-out(.5)",
    );
    close(
        Easing::css(0.2, 0.0, 0.0, 1.0).map(0.25),
        0.607220362072682,
        tol,
        "standard(.25)",
    );
    close(
        Easing::css(0.68, -0.55, 0.265, 1.55).map(0.3),
        -0.049316718488667,
        tol,
        "in_out_back(.3)",
    );
}

#[test]
fn cubic_beziers_match_exact_and_gsap_custom_ease() {
    for case in &BEZIERS {
        let [x1, y1, x2, y2] = case.points;
        let ease = Easing::css(x1, y1, x2, y2);
        for (i, t) in T.iter().enumerate() {
            let got = ease.map(*t);
            // (1, 0, 0, 1) has x'(0.5) = 0: every u within 3e-6 of 0.5 gives
            // x(u) == 0.5 in f64, so the golden "exact" column (a bisection
            // on doubles) lands 3.1e-6 off there. The true value is 0.5 by
            // symmetry, which is what the solver answers.
            let exact_tol = if case.id == "css_in_out_expo" && *t == 0.5 {
                5e-6
            } else {
                1e-9
            };
            if exact_tol > 1e-9 {
                assert_eq!(got, 0.5);
            }
            close(
                got,
                case.exact[i],
                exact_tol,
                &format!("{} exact at {t}", case.id),
            );
            close(
                got,
                case.gsap[i],
                case.gsap_tol + 1e-9,
                &format!("{} gsap at {t}", case.id),
            );
        }
    }
}

#[test]
fn precise_bezier_solves_x_to_1e9() {
    // With y1 = 1/3 and y2 = 2/3 the curve's y(u) is u itself, so map(t) is the
    // solved curve parameter and x(u) can be checked directly.
    for (name, [x1, _, x2, _]) in CSS_PRESETS {
        let ease = Easing::css(x1, 1.0 / 3.0, x2, 2.0 / 3.0);
        let cx = 3.0 * x1;
        let bx = 3.0 * (x2 - x1) - cx;
        let ax = 1.0 - cx - bx;
        for k in 1..10_000 {
            let t = k as f64 / 10_000.0;
            let u = ease.map(t);
            let x = ((ax * u + bx) * u + cx) * u;
            assert!((x - t).abs() < 1e-9, "{name} at {t}: x(u) = {x}");
        }
    }
}

#[test]
fn css_clamps_x_and_keeps_y() {
    assert_eq!(
        Easing::css(-0.5, -2.0, 1.5, 3.0),
        Easing::CubicBezier {
            x1: 0.0,
            y1: -2.0,
            x2: 1.0,
            y2: 3.0
        }
    );
}

#[test]
fn parse_gsap_ease_names() {
    let p = |s| parse_gsap_ease(s);
    assert_eq!(p("power2.out"), Some(Easing::OutCubic));
    assert_eq!(p("power1"), Some(Easing::OutQuad));
    assert_eq!(p("power2.in"), Some(Easing::InCubic));
    assert_eq!(p("strong.inOut"), Some(Easing::InOutQuint));
    assert_eq!(p("quad.inOut"), Some(Easing::InOutQuad));
    assert_eq!(p("none"), Some(Easing::Linear));
    assert_eq!(p("linear"), Some(Easing::Linear));
    assert_eq!(p("power0"), Some(Easing::Linear));
    assert_eq!(p("sine.inOut"), Some(Easing::InOutSine));
    assert_eq!(p("circ"), Some(Easing::OutCirc));
    assert_eq!(p("bounce.in"), Some(Easing::InBounce));
    assert_eq!(
        p("back"),
        Some(Easing::Back {
            dir: EaseDir::Out,
            overshoot: 1.70158
        })
    );
    assert_eq!(
        p("back.out(2)"),
        Some(Easing::Back {
            dir: EaseDir::Out,
            overshoot: 2.0
        })
    );
    assert_eq!(
        p("elastic.inOut"),
        Some(Easing::Elastic {
            dir: EaseDir::InOut,
            amplitude: 1.0,
            period: 0.45
        })
    );
    assert_eq!(
        p("elastic"),
        Some(Easing::Elastic {
            dir: EaseDir::Out,
            amplitude: 1.0,
            period: 0.3
        })
    );
    assert_eq!(
        p("elastic.out(1.5, 0.5)"),
        Some(Easing::Elastic {
            dir: EaseDir::Out,
            amplitude: 1.5,
            period: 0.5
        })
    );
    assert_eq!(p("expo.in"), Some(Easing::Expo { dir: EaseDir::In }));
    assert_eq!(
        p("cubic-bezier(0.25,0.1,0.25,1)"),
        Some(Easing::css(0.25, 0.1, 0.25, 1.0))
    );
    assert_eq!(
        p(" cubic-bezier( 0.25, 0.1, 0.25, 1 ) "),
        Some(Easing::css(0.25, 0.1, 0.25, 1.0))
    );
    for bad in [
        "wobble",
        "",
        "power2.sideways",
        "cubic-bezier(1,2,3)",
        "back.out(x)",
        "sine(2)",
        "power1(",
        "expo.in)",
    ] {
        assert_eq!(p(bad), None, "{bad:?}");
    }
}

#[test]
fn power_maps_to_the_parity_arms() {
    assert_eq!(Easing::power(0, EaseDir::In), Easing::Linear);
    assert_eq!(Easing::power(1, EaseDir::Out), Easing::OutQuad);
    assert_eq!(Easing::power(2, EaseDir::Out), Easing::OutCubic);
    assert_eq!(Easing::power(3, EaseDir::InOut), Easing::InOutQuart);
    assert_eq!(Easing::power(4, EaseDir::In), Easing::InQuint);
}

#[test]
fn css_presets_are_the_25_rows_of_786() {
    assert_eq!(CSS_PRESETS.len(), 25);
    assert_eq!(CSS_PRESETS[0], ("linear", [0.0, 0.0, 1.0, 1.0]));
    assert_eq!(
        CSS_PRESETS[1],
        ("ease_in_sine", [0.470, 0.000, 0.745, 0.715])
    );
    assert_eq!(
        CSS_PRESETS[24],
        ("ease_in_out_back", [0.680, -0.550, 0.265, 1.550])
    );
    let mut names: Vec<&str> = CSS_PRESETS.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 25, "names are unique");
    // Every preset agrees with GSAP's golden cubic-bezier table.
    for (name, points) in CSS_PRESETS {
        let id = format!("css_{}", name.trim_start_matches("ease_"));
        let case = BEZIERS
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("{id}"));
        assert_eq!(case.points, points, "{name}");
    }
}

#[test]
fn css_points_round_trip_the_presets() {
    assert_eq!(Easing::Linear.css_points(), Some([0.0, 0.0, 1.0, 1.0]));
    assert_eq!(
        Easing::OutBack.css_points(),
        Some([0.175, 0.885, 0.320, 1.275])
    );
    assert_eq!(Easing::InOutExp.css_points(), Some([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(
        Easing::css(0.2, 0.0, 0.0, 1.0).css_points(),
        Some([0.2, 0.0, 0.0, 1.0])
    );
    for none in [
        Easing::OutElastic,
        Easing::OutBounce,
        Easing::Instant,
        Easing::Steps {
            n: 3,
            jump: Jump::End,
        },
    ] {
        assert_eq!(none.css_points(), None, "{none:?}");
    }
    for (name, points) in CSS_PRESETS {
        assert_eq!(
            Easing::css_preset(name).and_then(|e| e.css_points()),
            Some(points),
            "{name}"
        );
    }
    assert_eq!(Easing::css_preset("ease_sideways"), None);
}

fn half(t: f64) -> f64 {
    t * 0.5
}

fn double(t: f64) -> f64 {
    t * 2.0
}

#[test]
fn custom_ease_eq_uses_fn_addr_eq() {
    let a = Easing::Custom(CustomEase(half));
    assert_eq!(a, Easing::Custom(CustomEase(half)));
    assert_ne!(a, Easing::Custom(CustomEase(double)));
    assert_ne!(a, Easing::Linear);
    assert_eq!(a.map(0.5), 0.25);
    assert_ne!(Easing::Constant(0.2), Easing::Constant(0.3));
    assert_ne!(Easing::InQuad, Easing::OutQuad);
    assert_eq!(
        Easing::Expo { dir: EaseDir::In },
        Easing::Expo { dir: EaseDir::In }
    );
}

#[test]
fn default_is_out_quad() {
    assert_eq!(Easing::default(), Easing::OutQuad);
}

#[test]
fn smooth_steps() {
    assert_eq!(Easing::SmoothStep.map(0.5), 0.5);
    assert_eq!(Easing::SmoothStep.map(-1.0), 0.0);
    assert_eq!(Easing::SmoothStep.map(2.0), 1.0);
    assert_eq!(Easing::SmootherStep.map(0.5), 0.5);
    assert_eq!(Easing::SmootherStep.map(1.5), 1.0);
    close(
        Easing::SmoothStep.map(0.25),
        0.15625,
        0.0,
        "smoothstep(.25)",
    );
}
