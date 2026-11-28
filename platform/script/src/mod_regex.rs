use crate::heap::*;
use crate::makepad_live_id::live_id::*;
use crate::native::*;
use crate::value::*;
use crate::*;

pub fn define_regex_module(heap: &mut ScriptHeap, native: &mut ScriptNative) {
    let std = heap.module(id!(std));

    // regex(pattern, flags) -> ScriptRegex
    native.add_method(
        heap,
        std,
        id_lut!(regex),
        script_args_def!(pattern = NIL, flags = NIL),
        |vm, args| {
            let pattern_val = script_value!(vm, args.pattern);
            let flags_val = script_value!(vm, args.flags);

            let pattern_str;
            let mut flags_str = String::new();

            // Extract pattern string
            if let Some(r) = vm.bx.heap.string_with(pattern_val, |_, s| s.to_string()) {
                pattern_str = r;
            } else {
                return script_err_type_mismatch!(
                    vm.bx.threads.cur_ref().trap,
                    "regex() pattern must be a string"
                );
            }

            // Extract flags string (default to empty)
            if !flags_val.is_nil() {
                if let Some(r) = vm.bx.heap.string_with(flags_val, |_, s| s.to_string()) {
                    flags_str = r;
                } else {
                    return script_err_type_mismatch!(
                        vm.bx.threads.cur_ref().trap,
                        "regex() flags must be a string"
                    );
                }
            }

            match vm.bx.heap.new_regex(&pattern_str, &flags_str) {
                Ok(val) => val,
                Err(e) => {
                    script_err_invalid_args!(
                        vm.bx.threads.cur_ref().trap,
                        "regex compile error: {}",
                        e
                    )
                }
            }
        },
    );

    // Type methods on regex values

    // regex.test(str) -> bool
    native.add_type_method(
        heap,
        ScriptValueType::REDUX_REGEX,
        id!(test),
        script_args_def!(str = NIL),
        |vm, args| {
            let sself = script_value!(vm, args.self);
            let str_val = script_value!(vm, args.str);

            if let Some(re_ptr) = sself.as_regex() {
                if let Some(result) = vm.bx.heap.string_with(str_val, |heap, s| {
                    heap.regex(re_ptr).map(|re| re.inner.run(s, &mut []))
                }) {
                    if let Some(matched) = result {
                        return matched.into();
                    }
                }
                return script_err_type_mismatch!(
                    vm.bx.threads.cur_ref().trap,
                    "regex.test() argument must be a string"
                );
            }
            script_err_unexpected!(vm.bx.threads.cur_ref().trap, "test called on non-regex")
        },
    );

    // regex.exec(str) -> object {value, index, captures} or nil
    native.add_type_method(
        heap,
        ScriptValueType::REDUX_REGEX,
        id!(exec),
        script_args_def!(str = NIL),
        |vm, args| {
            let sself = script_value!(vm, args.self);
            let str_val = script_value!(vm, args.str);

            if let Some(re_ptr) = sself.as_regex() {
                // We need to get num_captures first, then run
                let num_captures = if let Some(re) = vm.bx.heap.regex(re_ptr) {
                    re.num_captures
                } else {
                    return NIL;
                };

                let Some(num_slots) = crate::string::charge_regex_slots(&mut vm.bx.heap, num_captures, "running a regex") else {
                    return NIL;
                };
                let mut slots = vec![None; num_slots];
                let trap = vm.bx.threads.cur_ref().trap.pass();
                return vm.bx.heap.string_mut_self_with(str_val, |heap, input| {
                    if heap.regex(re_ptr).is_some_and(|re| re.inner.run(input, &mut slots)) {
                        crate::string::regex_match_from_slots(heap, input, &slots, trap)
                    } else {
                        NIL
                    }
                }).unwrap_or(NIL);
            }
            script_err_unexpected!(vm.bx.threads.cur_ref().trap, "exec called on non-regex")
        },
    );

    // Property getters: regex.source, regex.global
    native.set_type_getter(ScriptValueType::REDUX_REGEX, |vm, value, field| {
        if let Some(re_ptr) = value.as_regex() {
            if let Some(re) = vm.bx.heap.regex(re_ptr) {
                if field == id!(source) {
                    let pat = re.pattern.clone();
                    return vm.bx.heap.new_string_from_str(&pat);
                } else if field == id!(global) {
                    return re.flags.global.into();
                }
            }
        }
        NIL
    });
}
