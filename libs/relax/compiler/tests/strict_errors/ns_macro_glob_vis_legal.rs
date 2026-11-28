//~ none
mod bridge_pub { pub use crate::*; }
mod caller_pub {
    use crate::bridge_pub::*;
    pub fn call() { script_eval!(); }
}
mod bridge_priv {
    use crate::*;
    pub mod child {
        pub use super::*;
        pub fn call() { script_eval!(); target::script_eval!(); }
        pub mod grand {
            use super::*;
            pub fn call() { script_eval!(); }
        }
    }
}
mod bridge_self {
    pub(self) use crate::*;
    pub mod child {
        use super::*;
        pub fn call() { script_eval!(); }
    }
}
mod bridge_crate { pub(crate) use crate::*; }
mod caller_crate {
    use crate::bridge_crate::*;
    pub fn call() { script_eval!(); }
}
mod scope_super {
    pub mod bridge {
        pub(super) use crate::*;
        pub(super) use crate::target;
    }
    pub mod sibling {
        use super::bridge::*;
        pub fn call() { script_eval!(); target::script_eval!(); }
        pub mod deep {
            use super::super::bridge::*;
            pub fn call() { script_eval!(); }
        }
    }
}
mod scope_in {
    pub mod mid { pub mod bridge { pub(in crate::scope_in) use crate::*; } }
    pub mod other {
        use super::mid::bridge::*;
        pub fn call() { script_eval!(); }
    }
}
mod scope_in_super {
    pub mod mid { pub mod bridge { pub(in super::super) use crate::*; } }
    pub mod other {
        use super::mid::bridge::*;
        pub fn call() { script_eval!(); }
    }
}
pub mod pub_src { pub use crate::script_eval; }
pub mod widen {
    use crate::pub_src::*;
    pub use crate::pub_src::*;
    pub fn own() { script_eval!(); }
}
mod caller_widen {
    use crate::widen::*;
    pub fn call() { own(); script_eval!(); }
}
mod type_source { pub(crate) mod sub { pub use crate::script_eval; } }
mod type_caller {
    use crate::type_source::*;
    pub fn call() { sub::script_eval!(); }
}
mod ab { pub(crate) use std::stringify as my_str; }
mod ac {
    use crate::ab::*;
    pub fn call() -> &'static str { my_str!(ok) }
}
pub mod target { pub use crate::script_eval; }
#[macro_export]
macro_rules! script_eval { () => {} }
fn main() {
    caller_pub::call();
    bridge_priv::child::call();
    bridge_priv::child::grand::call();
    bridge_self::child::call();
    caller_crate::call();
    scope_super::sibling::call();
    scope_super::sibling::deep::call();
    scope_in::other::call();
    scope_in_super::other::call();
    caller_widen::call();
    type_caller::call();
    assert_eq!(ac::call(), "ok");
}
