// `?` in a closure without a declared return type: each `?` adds an `E: From<error>` obligation for the closure's
// error type E (rustc), the `?`s never equate their error types. makepad-motion-draw compile_inline:
// `eval(vm, &object)?.as_object().ok_or("..")?` (String, then &str) with E = String known only after the closure,
// from `match result { Err(error) => Some(error), .. }` -> Option<String>. "| " lines equal rustc's.
fn eval(ok: bool) -> Result<u32, String> {
    if ok { Ok(7) } else { Err("eval failed".to_string()) }
}

fn run(ok: bool, some: bool) -> Option<String> {
    let f = || {
        let v = eval(ok)?;
        let w = if some { Some(v + 1) } else { None }.ok_or("no value")?;
        let _ = w;
        Ok(())
    };
    let result = f();
    match result {
        Err(error) => Some(error),
        Ok(()) => None,
    }
}

fn first_str_then_string(k: u32) -> Result<u32, String> {
    let g = |k: u32| {
        let a = if k > 0 { Some(k) } else { None }.ok_or("zero")?;
        let b = if k < 10 { Ok(a * 2) } else { Err(format!("{} too big", k)) }?;
        Ok(b)
    };
    g(k)
}

#[test]
fn a01_closure_from() {
    println!("| {:?} {:?} {:?}", run(true, true), run(false, true), run(true, false));
    println!("| {:?} {:?} {:?}", first_str_then_string(3), first_str_then_string(0), first_str_then_string(12));
}
