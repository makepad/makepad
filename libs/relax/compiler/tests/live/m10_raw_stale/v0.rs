// M10: a value built by raw writes through addr_of_mut! (no offset_of!) in the memory a moved-out
// value of a struct literal left: its ext word is that value's (no block, every slot set). v1
// removes a field that needs dropping: the raw value never sets that dead slot, and a stale word
// would drop the bytes the moved-out value left there (a second drop of its Note) -- restart.

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

fn raw(name: &str, n: u32) -> Box<Rec> {
    unsafe {
        let p = alloc(Layout::new::<Rec>()).cast::<Rec>();
        addr_of_mut!((*p).name).write(name.to_string());
        addr_of_mut!((*p).note).write(Note(n));
        addr_of_mut!((*p).n).write(n);
        Box::from_raw(p)
    }
}

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
    let r = raw("r", 7);
    println!("out {} {} {} {}", k.name, k.n, r.name, r.n);
    drop(r);
    kept.push(Box::new(k));
    drop(kept);
    println!("out drops {}", DROPS.load(Ordering::SeqCst) - drops);
    println!("out ok");
}
