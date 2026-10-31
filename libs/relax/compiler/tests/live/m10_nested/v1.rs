use std::sync::Mutex;

struct Inner {
    n: u32,
    done: bool,
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
            items.push(Outer { tag, inner: [Inner { n: 10, done: false }, Inner { n: 20, done: false }] });
        }
    }
    for item in items.iter_mut() {
        println!("out {} {} {} {}", item.tag, item.inner[0].n, item.inner[1].n, item.total());
        println!("out done {} {}", item.inner[0].done, item.inner[1].done);
        item.inner[1].done = true;
        item.bump();
    }
}
