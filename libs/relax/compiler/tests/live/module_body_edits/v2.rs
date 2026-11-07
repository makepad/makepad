// module_body_edits: an inline module's body is its items. v1 edits an item macro's argument
// and an item its cfg leaves out; v2 makes a nested module public, calls into it from outside
// and adds a test fn to a test module; v3 makes the module private again: the restart rung.
macro_rules! konst {
    ($n:ident, $v:expr) => {
        pub fn $n() -> u64 {
            $v
        }
    };
}

mod outer {
    konst!(seven, 8);
    #[cfg(any())]
    fn never() -> u64 {
        2
    }
    pub mod hidden {
        pub fn deep() -> u64 {
            100
        }
    }
    pub fn via() -> u64 {
        hidden::deep() + 1
    }
}

#[cfg(test)]
mod checks {
    #[test]
    fn seven_is_small() {
        assert!(super::outer::seven() < 10);
    }

    #[test]
    fn deep_is_big() {
        assert!(super::outer::hidden::deep() > 10);
    }
}

#[test]
fn run() {
    println!("out {} {} {}", outer::seven(), outer::via(), outer::hidden::deep());
}
