static OTHER: relax_runner::Host = relax_runner::Host { tool: "other", ..makepad_run::HOST };
relax_runner::linked_host!(OTHER);

#[test]
fn initialization_reports_both_linked_hosts() {
    let panic = std::panic::catch_unwind(relax_base::host::init).expect_err("initializing a binary with two linked hosts must panic");
    let message = panic.downcast_ref::<String>().map(String::as_str).or_else(|| panic.downcast_ref::<&str>().copied()).expect("the panic must describe the linked hosts");
    assert!(message.contains("makepad") && message.contains("other"), "the panic must name both linked hosts: {message}");
}
