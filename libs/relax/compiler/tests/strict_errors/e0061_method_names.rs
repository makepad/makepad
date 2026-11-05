//~ E0061
//~ help: missing argument `scroll: f64`
struct Cx;
struct Walk;
impl Cx { fn draw(&mut self, walk: Walk, scroll: f64, count: usize) -> usize { let _ = (walk, scroll); count } }
fn main() { let mut cx = Cx; let _ = cx.draw(Walk, 3usize); }
