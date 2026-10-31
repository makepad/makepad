// M10: Inner gains a field inside [Inner; 2] inside Outer inside a Vec.
// Unchanged Outer methods must still read and mutate both array elements;
// two Outer values also exercise the Vec stride.

use std::sync::Mutex;

struct Inner {
    n: u32,
}

struct Outer {
    tag: u8,
    inner: [Inner; 2],
}

impl Outer {
    fn total(&self) -> u32 {
        self.tag as u32 + self.inner[0].n + self.inner[1].n
    }

    fn bump(&mut self) {
        self.inner[0].n += 1;
        self.inner[1].n += 2;
    }
}

static ITEMS: Mutex<Vec<Outer>> = Mutex::new(Vec::new());

#[test]
fn run() {
    let mut items = ITEMS.lock().unwrap();
    if items.is_empty() {
        for tag in 1..=2 {
            items.push(Outer { tag, inner: [Inner { n: 10 }, Inner { n: 20 }] });
        }
    }
    for item in items.iter_mut() {
        println!("out {} {} {} {}", item.tag, item.inner[0].n, item.inner[1].n, item.total());
        item.bump();
    }
}
