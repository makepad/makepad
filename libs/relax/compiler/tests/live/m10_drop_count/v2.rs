use std::sync::Mutex;

use std::sync::atomic::{AtomicUsize, Ordering};

static DROPS: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
struct Token {
    value: u32,
}

impl Drop for Token {
    fn drop(&mut self) {
        DROPS.fetch_add(1, Ordering::SeqCst);
    }
}

struct Item {
    name: String,
    n: u32,
    token: Token,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "a".into(), n: 10, token: Token::default() });
        items.push(Item { name: "b".into(), n: 20, token: Token::default() });
        items.push(Item { name: "new".into(), n: 40, token: Token { value: 7 } });
    }
    for item in items.iter_mut() {
        println!("out {} {}", item.name, item.n);
        item.n += 1;
    }
    println!("out tokens {} {} {}", items[0].token.value, items[1].token.value, items[2].token.value);
    println!("out before {}", DROPS.load(Ordering::SeqCst));
    items.clear();
    println!("out after {}", DROPS.load(Ordering::SeqCst));
    items.clear();
    println!("out again {}", DROPS.load(Ordering::SeqCst));
}
