// `use widgets::Rect` imports both namespaces of `Rect`: the value (a fn, from one glob of
// `widgets`) and the type (a struct, from another glob, reached through an import chain that
// resolves later than the value). rustc binds both; the import must not settle for the
// namespace it found first while a glob it looks through is still open.
#![allow(non_snake_case)]

pub mod host {
    use crate::widgets::Rect;

    #[derive(Clone, Debug, Default)]
    pub struct KeyButton {
        pub rect: Rect,
        pub remove: Rect,
    }

    thread_local! {
        static TIP: std::cell::RefCell<Option<(Rect, bool)>> = const { std::cell::RefCell::new(None) };
    }

    pub fn tip(rect: Rect) -> f64 {
        TIP.with(|t| *t.borrow_mut() = Some((rect, true)));
        TIP.with(|t| t.borrow().as_ref().map_or(0.0, |(r, _)| r.x))
    }

    pub fn style() -> u32 {
        Rect()
    }
}

pub mod widgets {
    pub use crate::style::*;
    pub use crate::draw::*;
}

pub mod style {
    pub fn Rect() -> u32 {
        7
    }
}

pub mod draw {
    pub use crate::platform::math::*;
}

pub mod platform {
    pub use crate::m3 as math;
}

pub use m2 as m3;
pub use m1 as m2;
pub use geometry as m1;

pub mod geometry {
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Rect {
        pub x: f64,
        pub w: f64,
    }
}

#[test]
fn use_imports_both_namespaces_through_late_globs() {
    let k = host::KeyButton { rect: geometry::Rect { x: 2.0, w: 3.0 }, ..Default::default() };
    let c = k.clone();
    assert_eq!(c.rect.x + c.rect.w + c.remove.w, 5.0);
    assert_eq!(host::tip(c.rect), 2.0);
    assert_eq!(host::style(), 7);
}
