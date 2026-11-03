//! The cross-thread interrupt: another thread stops a looping script, which bails uncatchably
//! ("script interrupted"), and the VM runs again once the flag is cleared.

use makepad_script::*;
use std::sync::mpsc;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new((), ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    vm
}

fn eval(vm: &mut ScriptVm, name: &str, code: &str) -> ScriptValue {
    // A limit the interrupt arrives far inside of; a missed interrupt ends on the limit instead.
    vm.with_instruction_limit(20_000_000, |vm| {
        vm.eval(ScriptMod::new("", "", &format!("interrupt_{name}.splash"), 0, 0, &format!("{code}\n;"), &[]))
    })
}

#[test]
fn another_thread_interrupts_a_looping_script() {
    let mut vm = test_vm();
    let handle = vm.bx.interrupt_handle();
    // The script tells the interrupter it is running; the interrupter stops it from its thread.
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let interrupter = std::thread::spawn(move || {
        started_rx.recv().unwrap();
        handle.interrupt();
    });
    let m = vm.new_module(id!(probe));
    vm.add_method(m, id_lut!(started), script_args_def!(), move |_, _| {
        let _ = started_tx.send(());
        NIL
    });

    let result = eval(&mut vm, "loop", "var n = 0\nmod.probe.started()\ntry { loop { n = n + 1 } } catch 99");
    interrupter.join().unwrap();
    assert!(result.is_err(), "{result:?}");
    let errors = vm.take_errors();
    assert!(errors.iter().any(|e| e.contains("script interrupted")), "{errors:#?}");
    assert!(!vm.thread().is_paused());

    // Still set: the next limited run stops too, until the flag is cleared.
    vm.bx.captured_errors = Some(Vec::new());
    assert!(eval(&mut vm, "still_set", "var n = 0\nloop { n = n + 1 }").is_err());
    let _ = vm.take_errors();
    vm.bx.interrupt_handle().clear();
    vm.bx.captured_errors = Some(Vec::new());
    assert_eq!(eval(&mut vm, "cleared", "var n = 0\nfor i in 0..5000 { n = n + 1 }\nn").as_number(), Some(5000.0));
    assert!(vm.take_errors().is_empty());
}
