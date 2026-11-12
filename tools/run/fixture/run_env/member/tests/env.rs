#[test]
fn an_integration_test_has_cargos_run_time_environment() {
    let mut bad = relax_run_env::check(true);
    // cargo hands an integration test its package's binaries at run time too
    let got = std::env::var("CARGO_BIN_EXE_relax-run-env").ok();
    if got.as_deref() != Some(env!("CARGO_BIN_EXE_relax-run-env")) {
        bad.push(format!("CARGO_BIN_EXE_relax-run-env: {got:?}, want {:?}", env!("CARGO_BIN_EXE_relax-run-env")));
    }
    assert!(bad.is_empty(), "{bad:#?}");
}
