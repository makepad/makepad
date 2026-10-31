use sc_fw::*;

fn main() {
    let r = [sc_mid::mid_area(), sc_mid::mid_shape_size() as u32, add(1, 2), flavor(), data() as u32, alt_val(), envv() as u32];
    let t: Vec<String> = r.iter().map(|x| x.to_string()).collect();
    println!("shared_apps result {}", t.join(" "));
}
