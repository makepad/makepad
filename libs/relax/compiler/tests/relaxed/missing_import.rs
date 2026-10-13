//! a module-level item used without its `use`
mod shapes {
    pub struct Circle { pub r: f32 }
    pub fn area(c: &Circle) -> f32 { 3.0 * c.r * c.r }
}
fn main() {
    let c: Circle = shapes::Circle { r: 2.0 };
    let mut m = HashMap::new();
    m.insert(1u8, area(&c));
    println!("{}", m[&1]);
}
