use makepad_script_std::{handle_script_tasks, makepad_script::*, queue_script_thread_resume, with_vm, ScriptStd};
use std::time::Duration;

#[test]
fn the_task_pump_resumes_the_selected_thread_under_its_policy() {
    let mut host = ScriptVmHost::new((), ScriptStd::default());
    host.script_vm = Some(Box::new(ScriptVmBase::new()));
    let thread = with_vm(&mut host.host_context(), |vm| {
        let mut policy = ScriptVmPolicy::untrusted();
        policy.fuel = Some(50);
        vm.bx.set_policy(policy).expect("install the task policy");
        vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, 2)));
        vm.eval(script! { for i in 0..1000 { i } });
        assert!(vm.run_yielded(), "the task suspended before its hard limit");
        vm.set_run_budget(None);
        vm.thread_id()
    });
    with_vm(&mut host.host_context(), |vm| {
        assert_ne!(vm.thread_id().to_index(), thread.to_index(), "ordinary work selects another thread while the task is suspended");
        assert_eq!(vm.eval(script!{ 2 + 2 }).as_number(), Some(4.), "the other entry completed");
    });
    queue_script_thread_resume(&mut host.std, thread);
    handle_script_tasks(&mut host.host_context());
    let vm = host.script_vm.as_ref().unwrap();
    assert_eq!(vm.poisoned(), Some(&Poison::Limit(Limit::Fuel)), "the task pump observed the resumed thread's fuel failure");
    let used = vm.policy().total();
    queue_script_thread_resume(&mut host.std, thread);
    handle_script_tasks(&mut host.host_context());
    assert!(host.std.data.tasks.pending_resumes.is_empty(), "a stopped task keeps no deferred resume");
    assert_eq!(host.script_vm.as_ref().unwrap().policy().total(), used, "the next pump did no guest work");
}
