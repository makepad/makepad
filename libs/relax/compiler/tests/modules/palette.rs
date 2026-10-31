use std::relax::{Edit, Module, Stream};

#[derive(Clone, Copy)]
pub struct Cell { pub light: f32 }

const SOURCE: &str = r#"
use test_crate::Cell;
struct Palette { red: f32, green: f32, blue: f32, generation: u32 }
fn palette() -> Palette {
    Palette { red: 0.125, green: 0.25, blue: 0.5, generation: 1 }
}
pub fn version() -> u32 { palette().generation }
pub fn color(c: &Cell) -> [f32; 4] {
    let p = palette();
    [p.red * c.light, p.green * c.light, p.blue * c.light, 1.0]
}
"#;

fn check(color: fn(&Cell) -> [f32; 4], version: fn() -> u32, generation: u32) {
    assert_eq!(version(), generation);
    let expected = if generation == 1 { [0.25, 0.5, 1.0, 1.0] } else { [1.5, 1.0, 0.25, 1.0] };
    assert_eq!(color(&Cell { light: 2.0 }), expected);
}

fn warm(color: fn(&Cell) -> [f32; 4], version: fn() -> u32, generation: u32) {
    // The default O3 threshold is 2000 calls. Leave time for the background
    // compiler, and check again after it could have installed its result.
    for _ in 0..10000 { std::hint::black_box(color(&Cell { light: 2.0 })); }
    std::thread::sleep(std::time::Duration::from_millis(1000));
    check(color, version, generation);
}

#[test]
fn palette_patch() {
    let new = SOURCE.replace("red: 0.125, green: 0.25, blue: 0.5, generation: 1",
                             "red: 0.75, green: 0.5, blue: 0.125, generation: 2");
    let m = Module::load_source("palette_patch", "memory/palette.rs", SOURCE, &["test_crate"]).unwrap();
    let color = m.get::<fn(&Cell) -> [f32; 4]>("color").unwrap();
    let version = m.get::<fn() -> u32>("version").unwrap();
    warm(color, version, 1);
    assert_eq!(m.replace(&new).unwrap(), Edit::Patched);
    check(color, version, 2);
    warm(color, version, 2);
    assert_eq!(m.replace(SOURCE).unwrap(), Edit::Patched);
    check(color, version, 1);
    warm(color, version, 1);
    assert_eq!(m.stream(&new).unwrap(), Stream::Applied { kept: 0 });
    check(color, version, 2);
    assert_eq!(m.finish_stream(&new).unwrap(), Edit::Patched);
    check(color, version, 2);
    warm(color, version, 2);
    m.unload();
    println!("out palette replace and stream: true");
}
