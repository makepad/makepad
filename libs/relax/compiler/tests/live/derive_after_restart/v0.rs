// M42: an edit adding a derived struct is rolled back (restart); the next edits
// must see that struct's derived impl again.

use std::sync::Mutex;

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
    println!("out {} {}", c.revision, c.beads.len());
}
