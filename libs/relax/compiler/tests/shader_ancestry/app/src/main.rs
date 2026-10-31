// The DrawShader derive pattern across crates and files (tests/shader_ancestry/run.sh).
mod shapes;
pub mod generated {
    include!("gen.rs");
}
use anc_wid::*;
use anc_draw::rr::{Depth, Layout};

// a base named by path from another module of this crate
#[derive(anc_mac::Shader)]
#[repr(C)]
struct Blur {
    #[deref]
    base: shapes::Gauss,
}

fn main() {
    let b = Blur { base: shapes::Gauss { base: anc_draw::Quad { x: 7 }, r: 3 } };
    let w = shapes::GaussWide { base: shapes::Gauss { base: anc_draw::Quad { x: 1 }, r: 2 } };
    println!(
        "ancestry ok {} {} {} {} {} {} {}",
        <anc_draw::Quad as Depth>::DEPTH,
        <shapes::Gauss as Depth>::DEPTH,
        <shapes::GaussWide as Depth>::DEPTH,
        <Blur as Depth>::DEPTH,
        <generated::Sharp as Depth>::DEPTH,
        b.base().base().x,
        w.base().base().x + w.base().r
    );
}
