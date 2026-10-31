#[derive(Debug, Clone)]
struct Thing { value: u32 }

fn edited() -> u32 { 1 }

#[test]
fn run() {
    let t = Thing { value: 7 };
    let c = t.clone();
    println!("out {:?} {}", c, edited());
}
