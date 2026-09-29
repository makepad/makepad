//! Sculpted lofted bodies cut by booleans (a race car's shell with its wheel
//! arches) must build: the kernel may retriangulate a smooth loft's
//! near-coplanar faces across their authored diagonals.
use makepad_model::{build_program, json, Limits};

fn run(path: &str) -> Result<usize, String> {
    let text = std::fs::read(path).map_err(|e| e.to_string())?;
    let ops = match json::parse(&text).map_err(|e| format!("{e:?}"))? { json::Value::Arr(ops) => ops, _ => return Err("program".into()) };
    let built = build_program(&ops, Limits::default(), &mut |_| Err("no assets".into()), None).map_err(|f| format!("op {} ({}): {}", f.index, f.op, f.message))?;
    Ok(built.compiled.glb.len())
}

#[test]
fn a_sculpted_car_shell_takes_its_wheel_arches() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sculpted_car_shell.json");
    let bytes = run(path).expect("the shell builds");
    assert!(bytes > 10_000);
}

/// `MODEL_REPLAY=program.json cargo test -p makepad-model --test sculpted -- --ignored`
#[test]
#[ignore]
fn replay() {
    let path = std::env::var("MODEL_REPLAY").expect("MODEL_REPLAY");
    println!("{:?}", run(&path));
}
