use crate::array::*;
use crate::heap::*;
use crate::makepad_live_id::live_id::*;
use crate::makepad_live_id_macros::*;
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

                let num_slots = (num_captures + 1) * 2;
                let mut slots = vec![None; num_slots];

                let matched = vm.bx.heap.string_with(str_val, |heap, s| {
                    if let Some(re) = heap.regex(re_ptr) {
                        if re.inner.run(s, &mut slots) {
                            Some(s.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });

                match matched {
                    Some(Some(input)) => {
                        let match_start = slots[0].unwrap_or(0);
                        let match_end = slots[1].unwrap_or(0);
                        let value = &input[match_start..match_end];

                        // Build result object
                        let obj = vm.bx.heap.new_with_proto(NIL);
                        let value_sv = vm.bx.heap.new_string_from_str(value);
                        let trap = vm.bx.threads.cur_ref().trap.pass();
                        vm.bx.heap.set_value(obj, id!(value).into(), value_sv, trap);
                        vm.bx.heap.set_value(
                            obj,
                            id!(index).into(),
                            ScriptValue::from_f64(match_start as f64),
                            trap,
                        );

                        // Build captures array
                        let captures = vm.bx.heap.new_array();
                        vm.bx
                            .heap
                            .array_mut_mut_self_with(captures, |heap, storage| {
                                *storage = ScriptArrayStorage::ScriptValue(Default::default());
                                if let ScriptArrayStorage::ScriptValue(vec) = storage {
                                    // Group 0 = whole match
                                    vec.push_back(heap.new_string_from_str(value));
                                    // Groups 1..N
                                    for i in 1..=num_captures {
                                        let s = slots.get(i * 2).copied().flatten();
                                        let e = slots.get(i * 2 + 1).copied().flatten();
                                        match (s, e) {
                                            (Some(s), Some(e)) => {
                                                vec.push_back(
                                                    heap.new_string_from_str(&input[s..e]),
                                                );
                                            }
                                            _ => vec.push_back(NIL),
                                        }
                                    }
                                }
                            });

                        vm.bx
                            .heap
                            .set_value(obj, id!(captures).into(), captures.into(), trap);
                        return obj.into();
                    }
                    _ => return NIL,
                }
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
