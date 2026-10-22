//! The flight recorder's records (FRAME summaries, EVENT, INPUT, owner service spans with their
//! site) decode, and a UI stall is answered from both sides.

use makepad_prof_stream::{self as ps, ask, Enc, Profile};

const MS: u64 = 1_000_000;

fn recording() -> Vec<u8> {
    let mut b = ps::prelude().to_vec();
    let start = 1_000 * MS;
    ps::record(&mut b, ps::kind::HELLO, &ps::Hello { app: "game".into(), host: String::new(), pid: 7, build: "b".into(), device: String::new(), refresh_mhz: 60_000, flags: 1, start_mono: start }.payload());
    let mut st = ps::Interner::default();
    for s in ["", "main", "run-owner", "Draw", "owner main", "serve background for run-cache-write at run.rs:2935", "compile", "hitch", "game.spawn", "run.args", "cache.write", "pump"] {
        st.id(s);
    }
    ps::record(&mut b, ps::kind::STRINGS, &st.take_new().unwrap());
    ps::record(&mut b, ps::kind::THREAD, &ps::thread_payload(1, 11, 0x100, ps::role::MAIN, 1));
    ps::record(&mut b, ps::kind::THREAD, &ps::thread_payload(2, 12, 0x200, ps::role::OWNER, 2));
    // at 896 s: the UI's frame waits 2967 ms on the owner, which serves a cache write meanwhile
    let t0 = start + 896_000 * MS;
    let main = [
        ps::SpanRec { kind: ps::span::BEGIN, t: t0, name: 3, a: 0 },
        ps::SpanRec { kind: ps::span::WAIT_BEGIN, t: t0 + MS, name: 4, a: 0 },
        ps::SpanRec { kind: ps::span::WAIT_END, t: t0 + 2968 * MS, name: 0, a: 0 },
        ps::SpanRec { kind: ps::span::END, t: t0 + 2970 * MS, name: 3, a: 0 },
    ];
    ps::record(&mut b, ps::kind::SPANS, &ps::spans_payload(1, &main));
    let owner = [
        ps::SpanRec { kind: ps::span::BEGIN, t: t0 - 10 * MS, name: 5, a: 0x300 },
        ps::SpanRec { kind: ps::span::BEGIN, t: t0 + 100 * MS, name: 6, a: 0 },
        ps::SpanRec { kind: ps::span::END, t: t0 + 400 * MS, name: 6, a: 0 },
        ps::SpanRec { kind: ps::span::END, t: t0 + 2960 * MS, name: 5, a: 0x300 },
        ps::SpanRec { kind: ps::span::SERVED, t: t0 + 2967 * MS, name: 0, a: 0x100 },
    ];
    ps::record(&mut b, ps::kind::SPANS, &ps::spans_payload(2, &owner));
    // frames: one ordinary, then the hitch (the flight recorder's summary fields appended)
    let mut prev = (0u64, 0u64);
    let mut fr: Vec<(u64, u64, u64, u64, u64)> = (0..6).map(|k| (5 + k, t0 - (120 - 20 * k) * MS, 16 * MS, 0, 0)).collect();
    fr.push((11, t0, 2970 * MS, 1, 2967 * MS));
    for (id, f0, dur, waits, owner_ns) in fr {
        let mut e = Enc::default();
        e.u(id - prev.0).d(f0, prev.1).u(dur).u(0).u(0).u(0);
        e.u(0).u(0).u(0).u(owner_ns).u(owner_ns).u(waits);
        prev = (id, f0);
        ps::record(&mut b, ps::kind::FRAME, &e.0);
    }
    // events and input
    let mut e = Enc::default();
    e.u(3).u(t0 - 500 * MS);
    e.d(t0 - 500 * MS, t0 - 500 * MS).u(0).u(9).u(0).u(0).s("/bin/makepad\u{1f}run\u{1f}game");
    e.d(t0 - 400 * MS, t0 - 500 * MS).u(1).u(8).u(42).u(0).s("follower 42 spawned");
    e.d(t0 + 2970 * MS, t0 - 400 * MS).u(1).u(7).u(2970 * MS).u(16 * MS).s("frame 2970 ms (typical 16 ms)");
    ps::record(&mut b, ps::kind::EVENT, &e.0);
    let mut e = Enc::default();
    e.u(2).u(t0 - 300 * MS);
    e.d(t0 - 300 * MS, t0 - 300 * MS).u(500).u(0).s("mouse k=down x=10 y=20 b=0 w=0");
    e.d(t0 - 290 * MS, t0 - 300 * MS).u(501).u(1).s("mouse k=up x=10 y=20 b=0 w=0");
    ps::record(&mut b, ps::kind::INPUT, &e.0);
    b
}

#[test]
fn decodes_flight_records() {
    let p = Profile::decode(&recording()).unwrap();
    assert_eq!(p.info.start_mono, 1_000 * MS);
    assert_eq!(p.frames.len(), 7);
    assert_eq!(p.frames[6].id, 11);
    assert_eq!(p.frames[6].owner_ns, 2967 * MS);
    assert_eq!(p.events.len(), 3);
    assert_eq!(p.inputs.len(), 2);
    assert!(p.inputs[1].injected && !p.inputs[0].injected);
}

#[test]
fn stall_names_both_sides() {
    let p = Profile::decode(&recording()).unwrap();
    let a = ask::ask(&p, "stall slowest", None).text();
    assert!(a.contains("at 896 s of the run the UI waited 2970 ms on owner main") || a.contains("at 896 s of the run the UI waited 2967 ms on owner main"), "{a}");
    assert!(a.contains("serving `serve background for run-cache-write at run.rs:2935`"), "{a}");
    assert!(a.contains("inside: compile 300 ms"), "{a}");
    let h = ask::ask(&p, "hitches", None).text();
    assert!(h.contains("F11: at 896 s"), "{h}");
    let e = ask::ask(&p, "marks", None).text();
    assert!(e.contains("follower 42 spawned") && !e.contains("run.args"), "{e}");
    let i = ask::ask(&p, "input", None).text();
    assert!(i.starts_with("2 input events"), "{i}");
    let at = ask::ask(&p, "at 897", None).text();
    assert!(at.contains("wait owner main") && at.contains("serve background"), "{at}");
}

/// `MKPF=<file> cargo test --release -p makepad-prof-stream --test flight -- --ignored ask_file`:
/// answers on a recording (Q="<query>", default brief).
#[test]
#[ignore]
fn ask_file() {
    let Ok(path) = std::env::var("MKPF") else { return };
    let p = Profile::decode(&std::fs::read(path).unwrap()).unwrap();
    let q = std::env::var("Q").unwrap_or_else(|_| "brief".into());
    for q in q.split(';') {
        println!("=== {q}\n{}", ask::ask(&p, q, None).text());
    }
}

/// `MKPF=<file> cargo test ... -- --ignored count_file`: segments per name and lane (what fills
/// the rings).
#[test]
#[ignore]
fn count_file() {
    let Ok(path) = std::env::var("MKPF") else { return };
    let p = Profile::decode(&std::fs::read(path).unwrap()).unwrap();
    let span = (p.t_max - p.t_min) as f64 / 1e9;
    let mut lanes: Vec<(usize, usize)> = (0..p.lanes.len()).map(|i| (i, p.lanes[i].rows.iter().map(|r| r.len()).sum::<usize>() + p.lanes[i].wakes.len())).collect();
    lanes.sort_by(|a, b| b.1.cmp(&a.1));
    println!("{} frames over {:.1} s", p.frames.len(), span);
    for (i, n) in lanes.iter().take(6) {
        let l = &p.lanes[*i];
        let mut by: std::collections::HashMap<(u32, bool), usize> = Default::default();
        for r in &l.rows {
            for s in r {
                *by.entry((s.name, s.wait)).or_default() += 1;
            }
        }
        let mut v: Vec<_> = by.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        println!("{} ({}): {} segs+wakes ({:.0}/s), wakes {}", p.lane_name(*i), l.thread.role, n, *n as f64 / span, l.wakes.len());
        for ((nm, w), c) in v.iter().take(12) {
            println!("   {:>7} {}{}", c, if *w { "wait " } else { "" }, p.s(*nm));
        }
    }
}
