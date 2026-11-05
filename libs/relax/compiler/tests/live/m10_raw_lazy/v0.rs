// M10: a builder by raw writes at offset_of! offsets that nothing calls before the edit, so it is
// first compiled after it. v1 removes a field that needs dropping and calls the builder in the
// memory a moved-out value of a struct literal left: its ext word is that value's, and would drop
// the bytes it left in the dead slot (a second drop of its Note) -- restart, decided from the
// program, not from what was compiled.

use std::alloc::{alloc, Layout};
use std::mem::offset_of;
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

#[allow(dead_code)]
fn raw(name: &str, n: u32) -> Box<Rec> {
    unsafe {
        let p = alloc(Layout::new::<Rec>());
        p.add(offset_of!(Rec, name)).cast::<String>().write(name.to_string());
        p.add(offset_of!(Rec, note)).cast::<Note>().write(Note(n));
        p.add(offset_of!(Rec, n)).cast::<u32>().write(n);
        Box::from_raw(p.cast::<Rec>())
    }
}

#[test]
fn run() {
    let kept = {
        let mut kept = KEPT.lock().unwrap();
        if kept.is_empty() {
            kept.push(Box::new(Rec { name: "k".to_string(), note: Note(100), n: 1 }));
        }
        kept.len()
    };
    println!("out kept {} drops {}", kept, DROPS.load(Ordering::SeqCst));
    println!("out ok");
}
