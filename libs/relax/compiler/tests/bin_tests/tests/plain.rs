// harness = false: cargo builds it with `--cfg test` but no `--test` and runs its main; the exit
// status is the verdict. `makepad test` runs it as a program of its own (run.rs --plain-test).
fn main() {
    println!("plain main {}", if cfg!(test) { "cfg-test" } else { "no-cfg-test" });
    assert_eq!(bt_app::add(1, 2), 3);
}
