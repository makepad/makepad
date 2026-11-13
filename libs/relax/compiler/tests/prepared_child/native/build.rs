// A native package (`links`, the build-dependency `cc`): the test's run builds it with cargo, from
// an input only the test's environment has. A run of the binary takes that build.
fn main() {
    println!("cargo:rerun-if-env-changed=PREPARED_TOKEN");
    assert_eq!(std::env::var("PREPARED_TOKEN").as_deref(), Ok("required"), "the native package built again without the test's input");
    cc::compile("csrc/native.c");
}
