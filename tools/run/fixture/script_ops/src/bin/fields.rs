//! A handle's fields from JIT-compiled code, with makepad-script bound to the loader's rustc
//! build: ScriptVmBase stays opaque (rustc's HashMap/Vec fields), but its plain fields are
//! reached at rustc's offsets (compute_program's driver sets `bail_on_uncaught_error` and
//! `allow_debug_output`, which was refused: "a field of ScriptVmBase is read from JIT-compiled code"),
//! and a handle field is a place the crate's ops take. A Vec field of plain data is the same bytes
//! on both sides when the two stds lay Vec out alike: ScriptTokenizer stays opaque (its lexer
//! state), but makepad-script-test's main reads its `tokens` (refused before: "a field of
//! ScriptTokenizer (`tokens: Vec<ScriptTokenPos>`) is reached from JIT-compiled code"); JIT code
//! grows rustc's Vec here, and rustc's ops read back what JIT code wrote and free it.
//! Prints what rustc's build prints (provided_script.sh compares the two).
use makepad_script::tokenizer::ScriptTokenizer;
use makepad_script::vm_ctl::{ScriptCtlReq, STAT_KEEP_NUMBERS};
use makepad_script::*;

#[derive(Script, ScriptHook)]
struct Named {
    #[live(2.5)]
    a: f64,
}

fn main() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let vm = &mut ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.bail_on_uncaught_error = true;
    vm.bx.allow_debug_output = false;
    vm.bx.keep_numbers = true;
    let keep = vm.bx.vm_stat(&mut ScriptCtlReq::new(STAT_KEEP_NUMBERS));
    Named::script_proto(vm);
    // `vm.bx.heap`: a handle field as a place, the heap's ops on it
    let named = vm.bx.heap.type_name_by_id(Named::script_type_id_static()).is_some();
    // `tok.tokens`: rustc's Vec read, grown past its capacity (a JIT realloc of rustc's block),
    // read back by rustc's `view` (tokens_ref), dropped by rustc's drop glue
    let mut tok = ScriptTokenizer::default();
    tok.tokenize("a + 1", &mut vm.bx.heap);
    let lexed = tok.tokens.len();
    let first = tok.tokens[0];
    for _ in 0..40 {
        tok.tokens.push(first);
    }
    let spaced = tok.tokens[1].preceded_by_space;
    println!("fields {} {} {} keep {} heap {} tokens {} {} {}", vm.bx.bail_on_uncaught_error, vm.bx.allow_debug_output, vm.bx.keep_numbers, keep, named, lexed, tok.tokens_ref().len(), spaced);
}
