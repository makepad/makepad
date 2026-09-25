//! Ticker: GSAP lag smoothing, global pause and time scale.

use makepad_tween::{ClockPolicy, LagSmoothing, TweenTicker};

#[test]
fn default_ticker() {
    let t = TweenTicker::default();
    assert_eq!(t.time_scale, 1.0);
    assert!(!t.paused);
    assert_eq!(t.lag, LagSmoothing::clamp(0.1));
    assert!(!t.reduced_motion);
    assert_eq!(t.epoch, 0);
    assert_eq!(
        ClockPolicy::default(),
        ClockPolicy {
            lag: None,
            first_dt: 0.0,
            follow_ticker: true
        }
    );
}

#[test]
fn dt_policies() {
    let t = TweenTicker::default();
    let p = ClockPolicy::default();
    assert_eq!(t.dt(None, p), 0.0);
    assert_eq!(
        t.dt(
            None,
            ClockPolicy {
                first_dt: 1.0 / 60.0,
                ..p
            }
        ),
        1.0 / 60.0
    );
    assert_eq!(t.dt(Some(0.016), p), 0.016);
    assert_eq!(t.dt(Some(0.25), p), 0.1);
    assert_eq!(t.dt(Some(-0.5), p), 0.0);

    let gsap = ClockPolicy {
        lag: Some(LagSmoothing::GSAP),
        ..p
    };
    assert_eq!(t.dt(Some(0.6), gsap), 0.033);
    assert_eq!(t.dt(Some(0.4), gsap), 0.4);
    assert_eq!(
        t.dt(
            Some(5.0),
            ClockPolicy {
                lag: Some(LagSmoothing::OFF),
                ..p
            }
        ),
        5.0
    );

    let paused = TweenTicker { paused: true, ..t };
    assert_eq!(paused.dt(Some(0.016), p), 0.0);
    assert_eq!(paused.dt(None, ClockPolicy { first_dt: 0.5, ..p }), 0.0);

    let fast = TweenTicker {
        time_scale: 2.0,
        ..t
    };
    assert_eq!(fast.dt(Some(0.05), p), 0.1);
    assert_eq!(fast.dt(Some(0.25), p), 0.2, "smoothed first, then scaled");

    // A clock that does not follow the ticker ignores pause, scale and lag.
    let own = ClockPolicy {
        follow_ticker: false,
        ..p
    };
    let odd = TweenTicker {
        paused: true,
        time_scale: 3.0,
        lag: LagSmoothing::OFF,
        ..t
    };
    assert_eq!(odd.dt(Some(0.05), own), 0.05);
    assert_eq!(odd.dt(Some(0.5), own), 0.1);
}

#[test]
fn lag_smoothing_filter() {
    assert_eq!(LagSmoothing::clamp(0.05).filter(0.2), 0.05);
    assert_eq!(LagSmoothing::clamp(0.05).filter(0.01), 0.01);
    assert_eq!(
        LagSmoothing::GSAP.filter(0.5),
        0.5,
        "the threshold itself passes"
    );
    assert_eq!(LagSmoothing::OFF.filter(-1.0), 0.0);
}
