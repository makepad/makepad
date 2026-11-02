// lazy_typeck.sh: `makepad run` checks the app's own crates whole before main; a dependency's body
// when it is first called, and every body in the background once main runs. The dependency
// (libs/dep: outside the app scope) has a type error in `broken`.
fn main() {
    println!("lazy-typeck: main runs");
    if std::env::args().nth(1).as_deref() == Some("call") {
        // parks at the call until a live edit fixes `broken`, then goes on
        println!("lazy-typeck: broken(2) = {}", lazy_typeck_dep::broken(2));
        return;
    }
    // `broken` never runs: the background check reports it meanwhile (the script stops the run)
    println!("lazy-typeck: fine(2) = {}", lazy_typeck_dep::fine(2));
    std::thread::sleep(std::time::Duration::from_secs(60));
}
