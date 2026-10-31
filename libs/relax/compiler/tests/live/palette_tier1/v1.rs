// A palette body edit must invalidate every caller that absorbed it, including O3.
struct Cell { light: f32 }
struct Palette { red: f32, green: f32, blue: f32, generation: u32 }
fn palette() -> Palette {
    Palette { red: 0.75, green: 0.5, blue: 0.125, generation: 2 }
}
pub fn version() -> u32 { palette().generation }
pub fn color(c: &Cell) -> [f32; 4] {
    let p = palette();
    [p.red * c.light, p.green * c.light, p.blue * c.light, 1.0]
}

#[test]
fn run() {
    let color = std::hint::black_box(color as fn(&Cell) -> [f32; 4]);
    let version = std::hint::black_box(version as fn() -> u32);
    let cell = Cell { light: 2.0 };
    println!("out immediate {} {:?}", version(), color(&cell));
    for _ in 0..10000 { std::hint::black_box(color(&cell)); }
    std::thread::sleep(std::time::Duration::from_millis(1000));
    println!("out warm {} {:?}", version(), color(&cell));
}
