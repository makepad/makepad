//! A text that starts at line 5 of its file (a `script_mod!` body, a document after a header)
//! has its parse errors and warnings on the lines its runtime errors have: row + 5. Both in the
//! `file:line:col` text and in the record's location, for a whole eval and a streamed one.

use makepad_script::*;

fn vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) }
}

/// The text line of a record (`lines.splash:6:12: ...` is 6).
fn text_line(record: &ScriptErrorRecord) -> u32 {
    record.text().split(':').nth(1).and_then(|l| l.parse().ok()).unwrap_or_else(|| panic!("no line in {:?}", record.text()))
}

fn record_line(record: &ScriptErrorRecord) -> u32 {
    record.loc().unwrap_or_else(|| panic!("no location: {:?}", record.text())).line()
}

#[test]
fn parse_errors_and_warnings_are_on_the_lines_runtime_errors_are() {
    // row 0 is line 5: the warning is on line 6, the error on line 7
    let code = "let a = 1\nlet v = [a -1]\nlet = 2\n";
    for streamed in [false, true] {
        let mut vm = vm();
        vm.capture_errors();
        vm.with_instruction_limit(100_000, |vm| {
            if streamed {
                vm.eval_with_append_source(ScriptMod::new("", "", "lines.splash", 5, 0, "", &[]), code, NIL.into());
            } else {
                vm.eval(ScriptMod::new("", "", "lines.splash", 5, 0, code, &[]));
            }
        });
        let warnings = vm.take_warning_records();
        let errors = vm.take_error_records();
        assert_eq!(warnings.len(), 1, "streamed {streamed}: {warnings:?}");
        assert_eq!((text_line(&warnings[0]), record_line(&warnings[0])), (6, 6), "streamed {streamed}: {:?}", warnings[0].text());
        let error = errors.iter().find(|e| e.text().starts_with("lines.splash:")).unwrap_or_else(|| panic!("streamed {streamed}: {errors:?}"));
        assert_eq!((text_line(error), record_line(error)), (7, 7), "streamed {streamed}: {:?}", error.text());
    }
    // A runtime error of the same text counts the same way.
    let mut vm = vm();
    vm.capture_errors();
    vm.with_instruction_limit(100_000, |vm| vm.eval(ScriptMod::new("", "", "lines.splash", 5, 0, "let a = 1\nlet b = nosuch(a)\n", &[])));
    let errors = vm.take_error_records();
    assert!(errors.iter().any(|e| record_line(e) == 6), "{:?}", errors.iter().map(|e| e.text()).collect::<Vec<_>>());
}

/// A text at line 0 keeps its records zero-based, as its runtime errors are, and its text
/// counts from 1.
#[test]
fn a_text_at_line_zero_counts_its_text_from_one() {
    let mut vm = vm();
    vm.capture_errors();
    vm.with_instruction_limit(100_000, |vm| vm.eval(ScriptMod::new("", "", "zero.splash", 0, 0, "let a = 1\nlet = 2\n", &[])));
    let errors = vm.take_error_records();
    let error = errors.iter().find(|e| e.text().starts_with("zero.splash:")).unwrap_or_else(|| panic!("{errors:?}"));
    assert_eq!((text_line(error), record_line(error)), (2, 1), "{:?}", error.text());
}

/// A fn's image and its placed tokens (what a kernel compiler reports its errors at) give its
/// line as runtime errors do: a fn on the text's second row of a ScriptMod at line 5 is on 6.
#[test]
fn a_fns_image_and_tokens_are_on_the_runtime_line() {
    let mut vm = vm();
    vm.capture_errors();
    let obj = vm.with_instruction_limit(100_000, |vm| vm.eval(ScriptMod::new("", "", "lines.splash", 5, 0, "let a = 1\n{f: fn(x) { return x + a }}\n", &[])));
    assert!(vm.take_errors().is_empty());
    let f = vm.value(obj.as_object().expect("the object"), LiveId::from_str("f")).as_object().expect("f");
    assert_eq!(vm.fn_image(f).expect("an image").line(), 6);
    let ip = vm.bx.heap.fn_script_ip(f).expect("a script fn");
    let tokens = vm.fn_tokens_placed(ip).expect("its tokens");
    assert_eq!((tokens.file.as_str(), tokens.tokens[0].line), ("lines.splash", 6));
}
