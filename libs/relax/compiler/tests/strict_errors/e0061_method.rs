//~ E0061
//~ help: missing arguments `walk: Walk`, `count: usize`
struct Cx;
struct Walk;
struct V;
impl V { fn draw(&self, cx: &mut Cx, walk: Walk, count: usize) { let _ = (cx, walk, count); } }
fn main() { let mut cx = Cx; V.draw(&mut cx); }
