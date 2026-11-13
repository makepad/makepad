// main runs, then calls code of its dependency that does not compile (modules_harness:
// a_fixture_whose_code_does_not_compile_fails_its_test)
fn main() {
    eprintln!("main ran");
    println!("{}", ce_dep::broken());
}
