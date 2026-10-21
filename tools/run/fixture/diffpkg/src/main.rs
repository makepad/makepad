// diff_fast package programs (tools/run/fixture/diff_fast.sh): `relax-diffpkg <program>` prints `D ...`
// lines; rustc --release and `makepad run` at each tier must print the same lines and exit alike.
// `relax-diffpkg --list` names the programs. Rounds as in the single-file programs: the work runs 4
// times with DIFF_SLEEP_MS between rounds so O3 code installs for the later rounds.
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }

#[path = "../../../../../libs/relax/compiler/rwbench/src/box3d_scene.rs"]
mod box3d_scene;

const PROGRAMS: &[(&str, fn())] = &[("box3d", box3d)];

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg == "--list" {
        for (n, _) in PROGRAMS {
            println!("{}", n);
        }
        return;
    }
    let Some((_, f)) = PROGRAMS.iter().find(|(n, _)| *n == arg) else {
        eprintln!("relax-diffpkg: unknown program {:?} (--list)", arg);
        std::process::exit(2);
    };
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 {
        f();
        if r < 3 && ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

/// The rwbench box3d scene at its small size: stacking, hinges with motor and limits, a rope, a spring
/// pendulum, falling mixed shapes. Per 20 steps the contact count and state hash, then every body.
fn box3d() {
    let mut s = box3d_scene::build(1);
    d!("box3d bodies={} contacts0={}", s.bodies.len(), box3d_scene::contact_count(&s));
    for k in 1..=120 {
        box3d_scene::step(&mut s);
        if k % 20 == 0 {
            d!("box3d step={} contacts={} state={:016x}", k, box3d_scene::contact_count(&s), box3d_scene::state_hash(&s));
        }
    }
    for i in 0..s.bodies.len() {
        d!("box3d body {} {:016x}", i, box3d_scene::body_hash(&s, i));
    }
}
