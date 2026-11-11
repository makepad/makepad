// M10: a builder by raw writes through addr_of_mut! that a macro_rules makes: its code is the
// expansion's, not the text at the call (read parsed). v1 removes a field: a value it made may be
// kept where the struct's type does not show, which its new layout would read wrong -- restart.

use std::alloc::{alloc, Layout};
use std::ptr::addr_of_mut;

struct Rec {
    name: String,
    tag: u32,
    n: u32,
}

macro_rules! raw_builder {
    ($f:ident -> $t:ty { $($field:ident = $v:expr),* }) => {
        fn $f() -> Box<$t> {
            unsafe {
                let p = alloc(Layout::new::<$t>()).cast::<$t>();
                $(addr_of_mut!((*p).$field).write($v);)*
                Box::from_raw(p)
            }
        }
    };
}

raw_builder!(raw -> Rec { name = "r".to_string(), tag = 8, n = 7 });

#[test]
fn run() {
    let k = Rec { name: "k".to_string(), tag: 2, n: 1 };
    let r = raw();
    println!("out {} {} {} {} {} {}", k.name, k.tag, k.n, r.name, r.tag, r.n);
    println!("out ok");
}
