//~ E0599
//~ help: methods with a similar name on this type: `draw_walk`
struct W;
impl W { fn draw_walk(&self) {} fn handle_event(&self) {} }
fn main() { W.draw_wlk(); }
