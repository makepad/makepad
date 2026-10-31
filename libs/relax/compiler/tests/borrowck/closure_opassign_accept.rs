// egui data/input.rs ModifierNames::format, epaint text_layout_types.rs Galley::concat and
// egui context.rs accesskit scroll: an overloaded `a op= b` inside a closure is a `&mut` borrow
// of the place (rustc_hir_typeck/src/op.rs:283-291), so the closure is FnMut, captures the place
// by unique borrow, and may be called repeatedly or write through a captured `&mut`.
use std::ops::{AddAssign, BitOrAssign};

#[derive(Clone, Copy, Debug)]
struct Rect(f32);
impl BitOrAssign for Rect { fn bitor_assign(&mut self, o: Rect) { self.0 = self.0.max(o.0); } }
#[derive(Clone, Copy, Debug, Default)]
struct V(f32);
impl AddAssign for V { fn add_assign(&mut self, o: V) { self.0 += o.0; } }

struct Names<'a> { concat: &'a str, ctrl: &'a str, shift: &'a str }
impl Names<'_> {
    fn format(&self, ctrl: bool, shift: bool) -> String {
        let mut s = String::new();
        let mut append_if = |active, name| {
            if active {
                if !s.is_empty() {
                    s += self.concat;
                }
                s += name;
            }
        };
        append_if(ctrl, self.ctrl);
        append_if(shift, self.shift);
        s
    }
}

struct G { rows: Vec<u32>, rect: Rect, bounds: Rect }
fn concat(src: &[u32]) -> G {
    let mut g = G { rows: Vec::new(), rect: Rect(0.0), bounds: Rect(0.0) };
    g.rows.extend(src.iter().map(|r| {
        g.bounds |= Rect(*r as f32);
        g.rect |= Rect(*r as f32 * 2.0);
        *r
    }));
    g
}

#[derive(Default)] struct Pass { delta: (V, u8) }
#[derive(Default)] struct Input { n: u32 }
impl Input { fn consume(&mut self, mut f: impl FnMut(u32) -> bool) -> bool { self.n += 1; f(self.n) } }
#[derive(Default)] struct Vp { input: Input, pass: Pass }
struct Ctx { vp: Vp }
impl Ctx { fn viewport_for(&mut self) -> &mut Vp { &mut self.vp } }
fn scroll(ctx: &mut Ctx) -> bool {
    let viewport = ctx.viewport_for();
    viewport.input.consume(|n| {
        match n { 1 => { viewport.pass.delta.0 += V(100.0); } _ => return false }
        true
    })
}

fn main() {
    let n = Names { concat: "+", ctrl: "Ctrl", shift: "Shift" };
    println!("{}", n.format(true, true));
    println!("{:?}", concat(&[1, 2, 3]).rect);
    println!("{}", scroll(&mut Ctx { vp: Vp::default() }));
}
