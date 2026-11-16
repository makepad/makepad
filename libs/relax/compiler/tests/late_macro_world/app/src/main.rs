// The program expands `value!(0)` at its load; under the runner a run-time module then expands
// `value!(1)`, which builds the macro world again and calls the macro's `late_helper` for the first
// time. Prints `answer 42`.
pub use lmw_mac::value;
const INITIAL: u32 = value!(0);

fn main() {
    // (the harness edits this line between two runs: the program's expansions run again)
    std::hint::black_box(1u32);
    assert_eq!(INITIAL, 0);
    #[cfg(relax)]
    let answer = {
        let module = std::relax::Module::load_source("lmw_late", "memory/lmw_late.rs", "pub fn answer() -> u32 { lmw_app::value!(1) }", &["lmw_app"]).unwrap();
        module.get::<fn() -> u32>("answer").unwrap()()
    };
    #[cfg(not(relax))]
    let answer = value!(1);
    println!("answer {answer}");
}
