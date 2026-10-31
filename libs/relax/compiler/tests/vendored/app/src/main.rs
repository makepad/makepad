// A registry dependency resolved from the vendored directory (tests/vendored/run.sh).
fn main() {
    println!("vendored ok {} {}", tinydep::answer(), tinydep::extra());
}
