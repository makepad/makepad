use relax_runner::relax_native_macros;

#[test]
fn codegen_identity_includes_the_host_before_startup() {
    let before = relax_compiler::codegen_id::id();
    assert!(relax_native_macros::sets().iter().any(|set| set.name == "makepad"), "the first codegen identity read must include makepad's native macros");
    relax_runner::start_host(&makepad_run::HOST);
    assert_eq!(relax_compiler::codegen_id::id(), before, "the codegen identity must not change once the host starts");
}
