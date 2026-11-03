// M42: an edit adding a derived struct is rolled back (restart); the next edits
// must see that struct's derived impl again.

use std::sync::Mutex;

#[derive(Clone)]
struct Contours {
    all: Vec<u32>,
    top: u32,
}

#[derive(Clone)]
struct Live {
    revision: u64,
    beads: Vec<u32>,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

#[test]
fn run() {
    let mut l = LIVE.lock().unwrap();
    let c = l.get_or_insert_with(|| Live { revision: 1, beads: vec![1, 2] }).clone();
    let k = Contours { all: c.beads.clone(), top: 4 }.clone();
    println!("out {} {}", c.revision, k.all.len() + k.top as usize);
}
