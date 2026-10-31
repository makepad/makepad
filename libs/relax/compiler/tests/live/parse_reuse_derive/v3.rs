#[derive(Debug, Clone)]
struct Thing { value: u32 }

fn edited() -> u32 { 3 }

#[test]
fn run() {
    let t = Thing { value: 7 };
    let c = t.clone();
    println!("out {:?} {}", c, edited());
}
