#[derive(Debug, Clone, Default)]
struct Thing { value: u32 }

fn edited() -> u32 { 4 }

#[test]
fn run() {
    let t = Thing::default();
    let c = t.clone();
    println!("out {:?} {}", c, edited());
}
