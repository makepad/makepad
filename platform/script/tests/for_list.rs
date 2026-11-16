//! A `for` in value position is the list of its body's values (`let xs = for i in 0..3 { i * 2 }`),
//! as `[for ..]` is; a `for` statement stays a loop.

use makepad_script::*;

fn eval(code: &str) -> (ScriptValue, Vec<String>, ScriptVm<'static>) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(500_000, |vm| vm.eval(ScriptMod::new("", "", "for_list", 0, 0, &code, &[])));
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    (value, errs, vm)
}

fn numbers(code: &str) -> Vec<f64> {
    let (value, errs, vm) = eval(code);
    assert!(errs.is_empty(), "{code}: {errs:?}");
    let a = value.as_array().unwrap_or_else(|| panic!("{code}: not a list: {value:?}"));
    (0..vm.bx.heap_ref().array_len(a)).map(|i| vm.bx.heap_ref().array_index(a, i, NoTrap).as_number().unwrap()).collect()
}

#[test]
fn a_for_value_is_a_list() {
    assert_eq!(numbers("let xs = for i in 0..3 { i * 2 }\nxs"), vec![0.0, 2.0, 4.0]);
    assert_eq!(numbers("fn f(n) { return for i in 0..n { let k = i + 1\n k * k } }\nf(3)"), vec![1.0, 4.0, 9.0]);
    assert_eq!(numbers("let xs = []\nfor i in 0..3 { xs.push(i) }\nxs"), vec![0.0, 1.0, 2.0]);
    // As the source's last statement.
    let (_, errs, _) = eval("let xs = for i in 0..3 { i * 2 }\n");
    assert!(errs.is_empty(), "{errs:?}");
    // As a call's argument, also of objects made from a prototype.
    assert_eq!(numbers("fn f(a, b) { return b }\nf(0, for i in 0..3 { i * 2 })"), vec![0.0, 2.0, 4.0]);
    assert_eq!(numbers("let P = {k: 0}\nfn f(a, b) { return for o in b { o.k } }\nf(0, for i in 0..3 { P{k: i} })"), vec![0.0, 1.0, 2.0]);
    assert_eq!(numbers("let P = {k: 0 j: 0}\nfn f(a, b) { return for o in b { o.k } }\nf(0, for i in 0..3 { P{k: i j: [i 0]} })"), vec![0.0, 1.0, 2.0]);
}

#[test]
fn loop_positions_count_iterations_whatever_the_source() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let seen: std::rc::Rc<std::cell::RefCell<Vec<Vec<u32>>>> = Default::default();
    let module = vm.new_module(id!(probe));
    let s = seen.clone();
    vm.add_method(module, id!(mark), script_args!(), move |vm, _| {
        let mut out = Vec::new();
        vm.loop_positions(&mut out);
        s.borrow_mut().push(out);
        NIL
    });
    let code = "use mod.probe.*\nmark()\nfor i in 2..4 { for v in [@a @b] { mark() } }\nfor k in 3 { if k == 1 { mark() } }\nlet o = {x: 1 y: 2}\nfor v in o { mark() }\n";
    vm.with_instruction_limit(500_000, |vm| vm.eval(ScriptMod::new("", "", "loop_positions", 0, 0, code, &[])));
    assert!(vm.take_errors().is_empty(), "the probe script runs");
    let seen = seen.borrow();
    assert_eq!(seen[0], Vec::<u32>::new(), "no loop: no positions");
    assert_eq!(seen[1..5].to_vec(), vec![vec![0, 0], vec![0, 1], vec![1, 0], vec![1, 1]], "a range's position counts from 0 (not its value 2), nested outermost first");
    assert_eq!(seen[5], vec![1], "a count loop's position is its iteration");
    assert_eq!(seen[6..8].to_vec(), vec![vec![0], vec![1]], "an object loop's position is its entry");
}
