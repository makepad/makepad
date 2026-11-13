// Two run-time modules: `plain` gives the program's unit its sandbox; `derived` uses the derive,
// so its load builds the macro world again (it went after the program's load) while that sandbox
// lives, and the world's code runs beside the module's twins. Prints `app 42 plain 42 derived 42`.
pub use mbw_mac::Answer;

#[derive(Answer)]
pub struct InApp;

fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/modules");
    let plain = std::relax::Module::load("mbw_plain", &format!("{dir}/plain.rs"), &[]).unwrap();
    let twice: fn(u32) -> u32 = plain.get("twice").unwrap();
    let derived = std::relax::Module::load("mbw_derived", &format!("{dir}/derived.rs"), &["mbw_app"]).unwrap();
    let answer: fn() -> u32 = derived.get("answer").unwrap();
    println!("app {} plain {} derived {}", InApp::answer(), twice(21), answer());
}
