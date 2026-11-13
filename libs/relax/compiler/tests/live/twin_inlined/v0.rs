// SPEC-V: code lowered before its verdict (RELAX_SPECV_HOLD keeps every verdict pending) sends its
// aliasing-prone calls (a `&mut` next to another pointer) to unverified-mode twins, and a twin's
// O3 build inlines the twins it calls. `run` calls `frame`'s twin, which is built at O3
// (RELAX_O3_CALLS=1 makes it hot at once; each run draws a few hundred frames and sleeps so that
// code installs) with `stamp`'s twin inlined. An edit of `stamp` replaces that copy too: the first
// frame after it runs the edited body.
//   v0: a stamp per grounded wheel
//   v1: and one for the body between them while two or more touch
use std::sync::Mutex;

pub struct Wheel {
    grounded: bool,
    radius: f32,
}

fn stamp(stamps: &mut Vec<f32>, wheels: &[Wheel]) {
    stamps.clear();
    for w in wheels {
        if w.grounded {
            stamps.push(0.18 + w.radius);
        }
    }
}

fn frame(stamps: &mut Vec<f32>, wheels: &[Wheel]) -> f32 {
    stamp(stamps, wheels);
    stamps.iter().sum()
}

static STATE: Mutex<Option<(Vec<f32>, Vec<Wheel>)>> = Mutex::new(None);

#[test]
fn run() {
    let mut held = STATE.lock().unwrap();
    let (stamps, wheels) = held.get_or_insert_with(|| {
        let wheel = |grounded, radius| Wheel { grounded, radius };
        (Vec::new(), vec![wheel(true, 0.35), wheel(false, 0.35), wheel(true, 0.4)])
    });
    let first = frame(stamps, wheels);
    println!("out first {} {:.3}", stamps.len(), first);
    for _ in 0..500 {
        frame(stamps, wheels);
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    let settled = frame(stamps, wheels);
    println!("out settled {} {:.3}", stamps.len(), settled);
}
