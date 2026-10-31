// rustc --edition=2021 --test fullcheck_relative_use.rs
// makepad test fullcheck_relative_use.rs
// In edition 2018+, an unprefixed use starts in the current scope. The
// view's child re-exports both a type and a same-named style function, and
// imports the parent scope back through a glob, as lowered .rx views do.

pub mod split {
    // A crate-root decoy: resolving view's `use split::SplitPane` here
    // would give the wrong field and method types.
    pub struct SplitPane {
        pub unrelated: bool,
    }
}

mod split_pane {
    pub struct DrawQuad {
        pub drawn: u32,
    }

    impl DrawQuad {
        pub fn draw_abs(&mut self, amount: u32) {
            self.drawn += amount;
        }
    }

    pub struct SplitPane {
        pub cam_time: f64,
        pub draw_bg: DrawQuad,
    }

    #[allow(non_snake_case)]
    pub fn SplitPane() -> SplitPane {
        SplitPane { cam_time: 1.0, draw_bg: DrawQuad { drawn: 0 } }
    }
}

mod view {
    // Put the consumer first so import resolution must reach a fixed point.
    use split::SplitPane;

    pub struct View {
        pane: SplitPane,
    }

    pub fn exercise() -> (f64, u32) {
        let mut view = View { pane: SplitPane() };
        view.draw();
        (view.pane.cam_time, view.pane.draw_bg.drawn)
    }

    mod split {
        use super::*;
        pub(super) use crate::split_pane::SplitPane;

        impl View {
            pub fn draw(&mut self) {
                let Self { pane } = self;
                pane.cam_time += 2.0;
                pane.draw_bg.draw_abs(7);
            }
        }
    }
}

mod grouped {
    use split::{self as pane_module, SplitPane as Pane};

    mod split {
        pub(super) use crate::split_pane::SplitPane;
    }

    pub fn exercise() -> f64 {
        let pane: Pane = pane_module::SplitPane();
        pane.cam_time
    }
}

#[test]
fn relative_child_reexport_preserves_type_and_value_namespaces() {
    assert_eq!(view::exercise(), (3.0, 7));
}

#[test]
fn grouped_relative_use_and_self_alias_name_the_child() {
    assert_eq!(grouped::exercise(), 1.0);
}

#[test]
fn block_relative_use_names_the_local_module() {
    use split::SplitPane;

    mod split {
        pub(super) use crate::split_pane::SplitPane;
    }

    let mut pane: SplitPane = SplitPane();
    pane.draw_bg.draw_abs(5);
    assert_eq!(pane.cam_time, 1.0);
    assert_eq!(pane.draw_bg.drawn, 5);
}
