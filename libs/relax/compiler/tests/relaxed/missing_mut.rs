struct P { x: u32 }
fn main() {
    let x = 1; x = 2;
    let v = Vec::<u8>::new(); v.push(1);
    let p = P { x: 0 }; p.x = 3;
    println!("{} {} {}", x, v.len(), p.x);
}
