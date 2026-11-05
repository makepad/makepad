// M10: a builder by raw writes through addr_of_mut! that a macro_rules makes: its code is the
// expansion's, not the text at the call (read parsed). It builds in the memory a moved-out value
// of a struct literal left, whose ext word it keeps. v1 removes a field that needs dropping: a
// stale word would drop the bytes the moved-out value left in the dead slot (a second drop of its
// Note) -- restart.

use std::alloc::{alloc, Layout};
use std::ptr::addr_of_mut;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

// owns something to drop, holds no niche
struct Note(u32);

static DROPS: AtomicU32 = AtomicU32::new(0);

impl Drop for Note {
    fn drop(&mut self) {
        DROPS.fetch_add(self.0, Ordering::SeqCst);
    }
}

struct Rec {
    name: String,
    note: Note,
    n: u32,
}

static KEPT: Mutex<Vec<Box<Rec>>> = Mutex::new(Vec::new());

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

raw_builder!(raw -> Rec { name = "r".to_string(), note = Note(7), n = 7 });

#[test]
fn run() {
    let drops = DROPS.load(Ordering::SeqCst);
    let mut kept = KEPT.lock().unwrap();
    if kept.is_empty() {
        kept.push(Box::new(Rec { name: "k".to_string(), note: Note(100), n: 1 }));
    }
    // moved out of its box, whose memory keeps the value's bytes (its ext word too)
    let k = *kept.pop().unwrap();
    // built where that box was
    let r = raw();
    println!("out {} {} {} {}", k.name, k.n, r.name, r.n);
    drop(r);
    kept.push(Box::new(k));
    drop(kept);
    println!("out drops {}", DROPS.load(Ordering::SeqCst) - drops);
    println!("out ok");
}
