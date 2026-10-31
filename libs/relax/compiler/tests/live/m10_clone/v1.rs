use std::sync::Mutex;

#[derive(Clone)]
struct Item {
    name: String,
    n: u32,
    done: bool,
}

static ITEMS: Mutex<Vec<Item>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        items.push(Item { name: "old".into(), n: 10, done: false });
    }
    println!("out original {} {}", items[0].name, items[0].n);
    println!("out default {}", items[0].done);
    items[0].done = true;
    let mut cloned = items[0].clone();
    println!("out cloned {} {} {}", cloned.name, cloned.n, cloned.done);
    cloned.done = false;
    cloned.name.push_str("-clone");
    println!("out independent {} {} {} {}", items[0].name, items[0].done, cloned.name, cloned.done);
    items[0].n += 1;
}
