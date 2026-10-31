//~ E0061
//~ help: missing argument `scroll: f64`
struct Cx;
struct Walk;
fn draw_list(cx: &mut Cx, walk: Walk, scroll: f64, count: usize) -> usize { let _ = (cx, walk, scroll); count }
fn main() { let mut cx = Cx; let _ = draw_list(&mut cx, Walk, 3usize); }
