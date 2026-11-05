// The macro world expands `#[derive(Probe)]` on the thread that loads this crate, before main runs
// there: its thread-locals have values on that thread then. The program's own (std's hash keys for
// the HashMap below) must start as a new thread's, never on the world's bytes: each unit placing its
// thread-locals from the block's start put this program's `KEYS` on the world's, and the first
// HashMap read an Option that was neither None nor Some (an illegal instruction at its match).
use macro_world_tls_mac::Probe;

pub trait Probe {
    fn fields() -> usize;
}

#[derive(Probe)]
struct Point {
    x: u32,
    y: u32,
    label: String,
}

fn main() {
    let p = Point { x: 1, y: 2, label: "p".to_string() };
    let mut m = std::collections::HashMap::new();
    m.insert("x", p.x);
    m.insert("y", p.y);
    m.insert("label", p.label.len() as u32);
    let on_thread = std::thread::spawn(|| {
        let mut n = std::collections::HashMap::new();
        n.insert(1u8, 2u8);
        n.len()
    }).join().unwrap();
    println!("macro world tls: fields {} map {} thread map {}", Point::fields(), m.len(), on_thread);
}
