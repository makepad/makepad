use crate::array::*;
use crate::heap::*;
use crate::makepad_live_id::*;
use crate::native::*;
use crate::value::*;
use crate::*;
use ::std::borrow::Borrow;
use ::std::sync::Arc;

#[derive(Clone, Default, PartialEq, Eq, Hash)]
pub struct ScriptRcString(pub Arc<String>);

impl ScriptRcString {
    pub fn new(str: String) -> Self {
        Self(Arc::new(str))
    }
}

impl Borrow<str> for ScriptRcString {
    fn borrow(&self) -> &str {
        (*self.0).as_str()
    }
}

impl Borrow<String> for ScriptRcString {
    fn borrow(&self) -> &String {
        &(*self.0)
    }
}

#[derive(Default)]
pub struct StringTag(u64);

impl StringTag {
    const MARK: u64 = 0x1;
    const STATIC: u64 = 0x2;

    pub fn is_marked(&self) -> bool {
        self.0 & Self::MARK != 0
    }

    pub fn set_mark(&mut self) {
        self.0 |= Self::MARK
    }

    pub fn clear_mark(&mut self) {
        self.0 &= !Self::MARK
    }

    pub fn set_static(&mut self) {
        self.0 |= Self::STATIC
    }

    pub fn is_static(&self) -> bool {
        self.0 & Self::STATIC != 0
    }
}

#[derive(Default)]
pub struct ScriptStringData {
    pub tag: StringTag,
    pub string: ScriptRcString,
}

impl ScriptStringData {
    pub fn add_type_methods(native: &mut ScriptNative, heap: &mut ScriptHeap) {
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(to_bytes),
            &[],
            |vm, args| {
                let sself = script_value!(vm, args.self);
                vm.bx.heap.string_to_bytes_array(sself).into()
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(to_chars),
            &[],
            |vm, args| {
                let sself = script_value!(vm, args.self);
                vm.bx.heap.string_to_chars_array(sself).into()
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(to_f64),
            &[],
            |vm, args| {
                let sself = script_value!(vm, args.self);
                if let Some(r) = vm.bx.heap.string_mut_self_with(sself, |_heap, s| {
                    ScriptValue::from_f64(s.parse().unwrap_or(f64::NAN))
                }) {
                    r
                } else {
                    ScriptValue::from_f64_traced_nan(f64::NAN, vm.bx.threads.cur_ref().trap.ip)
                }
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(parse_json),
            &[],
            |vm, args| {
                let sself = script_value!(vm, args.self);

                // Extract json_parser temporarily to avoid borrow conflict
                let mut json_parser = std::mem::take(&mut vm.bx.threads.cur().json_parser);
                let result = if let Some(r) = vm
                    .bx
                    .heap
                    .string_mut_self_with(sself, |heap, s| json_parser.read_json(s, heap))
                {
                    r
                } else {
                    script_err_unexpected!(
                        vm.bx.threads.cur_ref().trap,
                        "parse_json called on non-string value"
                    )
                };
                vm.bx.threads.cur().json_parser = json_parser;
                result
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(trim),
            script_args_def!(),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                if let Some(s) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    heap.new_string_from_str(sself.trim())
                }) {
                    return s.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "trim called on non-string value"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(strip_prefix),
            script_args_def!(pat = NIL),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                let pat = script_value!(vm, args.pat);
                if let Some(Some(s)) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    heap.string_mut_self_with(pat, |heap, pat| {
                        heap.new_string_from_str(if let Some(s) = sself.strip_prefix(pat) {
                            s
                        } else {
                            sself
                        })
                    })
                }) {
                    return s.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "strip_prefix requires string arguments"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(strip_suffix),
            script_args_def!(pat = NIL),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                let pat = script_value!(vm, args.pat);
                if let Some(Some(s)) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    heap.string_mut_self_with(pat, |heap, pat| {
                        heap.new_string_from_str(if let Some(s) = sself.strip_suffix(pat) {
                            s
                        } else {
                            sself
                        })
                    })
                }) {
                    return s.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "strip_suffix requires string arguments"
                )
            },
        );

        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(split),
            script_args_def!(pat = NIL),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                let pat = script_value!(vm, args.pat);
                if let Some(Some(s)) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    heap.string_mut_self_with(pat, |heap, pat| {
                        let array = heap.new_array();
                        heap.array_mut_mut_self_with(array, |heap, storage| {
                            if let ScriptArrayStorage::ScriptValue(_) = storage {
                            } else {
                                *storage = ScriptArrayStorage::ScriptValue(Default::default());
                            }
                            if let ScriptArrayStorage::ScriptValue(vec) = storage {
                                vec.clear();
                                for s in sself.split(pat) {
                                    vec.push_back(heap.new_string_from_str(s));
                                }
                            }
                        });
                        array
                    })
                }) {
                    return s.into();
                }

                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "split requires string arguments for both self and pattern"
                )
            },
        );
    }
}
