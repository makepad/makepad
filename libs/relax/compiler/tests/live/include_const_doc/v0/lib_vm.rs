//! The VM side: values read back, then the library the kernels are compiled from.
use std::cell::RefCell;

/// A value read back.
#[derive(Clone, Debug)]
pub enum V {
    Num(f64),
    Text(String),
}

impl V {
    /// What it reads as: macro calls in a body above the const.
    pub fn show(&self) -> String {
        match self {
            V::Num(n) => format!("{n}"),
            V::Text(s) => format!("\"{s}\""),
        }
    }
}

/// What one evaluation keeps.
#[derive(Default, Debug)]
pub struct Ctx {
    pub keep: Vec<u32>,
    pub texture: Option<u32>,
}

/// What the person reads first.
pub const GUIDE: &str = include_str!("guide.md");

/// The patterns and mappings the kernels are compiled from.
pub const TEXTURE_LIB: &str = include_str!("texture.splash");

pub type Shared = std::rc::Rc<RefCell<Ctx>>;

pub fn lib_text() -> String {
    let ctx: Shared = Default::default();
    ctx.borrow_mut().keep.push(1);
    format!("{} {}", TEXTURE_LIB.trim(), V::Num(ctx.borrow().keep.len() as f64).show())
}

pub fn count() -> usize {
    let v = vec![V::Text(TEXTURE_LIB.to_string())];
    v.len() + TEXTURE_LIB.lines().count()
}

pub fn guide() -> &'static str {
    GUIDE.trim()
}
