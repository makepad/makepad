// lazy_typeck.sh: `makepad run` checks the app's own crates whole before main; a dependency's body
// when it is first called, and every body in the background once main runs. Outside the app scope,
// libs/dep has a type error in `broken` and libs/store lacks a method required by libs/dep's trait.
// The trait modes call through that impl once or keep calling it while the trait is edited.
fn main() {
    println!("lazy-typeck: main runs");
    let mode = std::env::args().nth(1);
    if mode.as_deref() == Some("exit") {
        return;
    }
    if mode.as_deref() == Some("panic") {
        panic!("lazy-typeck: unwatched panic");
    }
    if mode.as_deref() == Some("call") {
        // parks at the call until a live edit fixes `broken`, then goes on
        println!("lazy-typeck: broken(2) = {}", lazy_typeck_dep::broken(2));
        return;
    }
    if mode.as_deref() == Some("trait") {
        println!("lazy-typeck: first = {}", lazy_typeck_store::first_of(&lazy_typeck_store::Store(vec![3])));
        return;
    }
    if mode.as_deref() == Some("trait-loop") {
        for i in 0..600 {
            println!("lazy-typeck: tick {i} first = {}", lazy_typeck_store::first_of(&lazy_typeck_store::Store(vec![3])));
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        return;
    }
    // `broken` never runs: the background check reports it meanwhile (the script stops the run)
    println!("lazy-typeck: fine(2) = {}", lazy_typeck_dep::fine(2));
    std::thread::sleep(std::time::Duration::from_secs(60));
}
