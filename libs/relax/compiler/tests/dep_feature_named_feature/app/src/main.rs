fn main() {
    let (n, d): df_facade::WideRatio = df_facade::wide_ratio(1, 2);
    println!("{} / {}", n.label(), d.label());
}
