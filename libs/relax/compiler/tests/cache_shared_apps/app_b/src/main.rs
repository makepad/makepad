use sc_fw::*;

fn own_b(x: u32) -> u32 { x * 7 + 1 }

#[derive(Debug)]
struct Mine { x: u32 }
fn mine_make() -> Mine { Mine { x: 1 } } // EDIT mine
impl Default for Mine { fn default() -> Mine { mine_make() } }

fn main() {
    let s = Shape::new(3, 5);
    let r = [sc_fw::flavor(), sc_fw::envv() as u32, sc_fw::data() as u32, sc_fw::alt_val(), sc_fw::wrap::<Mine>() as u32, add(2, 3), scaled(4u32) as u32, area(&s), describe(&s) as u32, cfg_macro(), gated(), uses_unrelated(), sum_to(10), pow_std(3), from_macro(), own_b(6)];
    let t: Vec<String> = r.iter().map(|x| x.to_string()).collect();
    println!("shared_apps result {}", t.join(" "));
}
