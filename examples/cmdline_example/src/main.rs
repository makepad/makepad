fn main() {
    let i = 1.0;
    for i in 0..32{
        let x = (i as f32).to_bits();
        println!("{}=>0x{:08x},", i, x);
    }
}