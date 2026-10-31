use std::sync::Mutex;

use std::sync::atomic::{AtomicUsize, Ordering};

static DROPS: AtomicUsize = AtomicUsize::new(0);

struct Token;

impl Drop for Token {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Item {
    name: String,
    n: u32,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "a".into(), n: 10 });
        items.push(Item { name: "b".into(), n: 20 });
        items.push(Item { name: "c".into(), n: 30 });
    }
    for item in items.iter_mut() {
        println!("out {} {}", item.name, item.n);
        item.n += 1;
    }
    println!("out before {}", DROPS.load(Ordering::SeqCst));
    items.clear();
    println!("out after {}", DROPS.load(Ordering::SeqCst));
}
