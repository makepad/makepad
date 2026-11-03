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
    contours: Contours,
}

static LIVE: Mutex<Option<Live>> = Mutex::new(None);

#[test]
fn run() {
    let mut l = LIVE.lock().unwrap();
    let c = l.get_or_insert_with(|| Live { revision: 1, contours: Contours { all: vec![1, 2], top: 4 } }).clone();
    println!("out {} {}", c.revision, c.contours.all.len() + c.contours.top as usize);
}
