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

                // Regex path
                if let Some(re_ptr) = pat.as_regex() {
                    let result = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                        regex_split(heap, sself, re_ptr)
                    });
                    if let Some(arr) = result {
                        return arr.into();
                    }
                    return script_err_unexpected!(
                        vm.bx.threads.cur_ref().trap,
                        "split: self must be a string"
                    );
                }

                // String path
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

        // str.search(pat) -> number (byte index of first match, or -1)
        // pat can be a string or regex
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(search),
            script_args_def!(pat = NIL),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                let pat = script_value!(vm, args.pat);

                // Regex path
                if let Some(re_ptr) = pat.as_regex() {
                    let result = vm.bx.heap.string_with(sself, |heap, s| {
                        if let Some(re) = heap.regex(re_ptr) {
                            let mut slots = [None; 2];
                            if re.inner.run(s, &mut slots) {
                                slots[0].unwrap_or(0) as f64
                            } else {
                                -1.0
                            }
                        } else {
                            -1.0
                        }
                    });
                    return ScriptValue::from_f64(result.unwrap_or(-1.0));
                }

                // String path
                if let Some(Some(result)) = vm.bx.heap.string_with(sself, |heap, sself| {
                    heap.string_with(pat, |_, pat| {
                        if let Some(idx) = sself.find(pat) {
                            idx as f64
                        } else {
                            -1.0
                        }
                    })
                }) {
                    return ScriptValue::from_f64(result);
                }
                ScriptValue::from_f64(-1.0)
            },
        );

        // str.match(pat) -> object or array or nil
        // If pat is a non-global regex: returns {value, index, captures} or nil
        // If pat is a global regex: returns array of matched strings
        // If pat is a string: returns {value, index} or nil
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(match_str),
            script_args_def!(pat = NIL),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                let pat = script_value!(vm, args.pat);

                // Regex path
                if let Some(re_ptr) = pat.as_regex() {
                    let (is_global, num_captures) = if let Some(re) = vm.bx.heap.regex(re_ptr) {
                        (re.flags.global, re.num_captures)
                    } else {
                        return NIL;
                    };

                    if is_global {
                        // Global: return array of all matched strings
                        let result = vm.bx.heap.string_mut_self_with(sself, |heap, s| {
                            regex_match_all_strings(heap, s, re_ptr)
                        });
                        if let Some(arr) = result {
                            return arr.into();
                        }
                        return NIL;
                    } else {
                        // Non-global: return first match detail
                        let result = vm.bx.heap.string_mut_self_with(sself, |heap, s| {
                            regex_exec_first(heap, s, re_ptr, num_captures)
                        });
                        if let Some(val) = result {
                            return val;
                        }
                        return NIL;
                    }
                }

                // String path: simple indexOf-style match
                if let Some(Some(result)) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    heap.string_mut_self_with(pat, |heap, pat| {
                        if let Some(idx) = sself.find(pat) {
                            let obj = heap.new_with_proto(NIL);
                            let value_sv = heap.new_string_from_str(pat);
                            heap.set_value_def(obj, id!(value).into(), value_sv);
                            heap.set_value_def(
                                obj,
                                id!(index).into(),
                                ScriptValue::from_f64(idx as f64),
                            );
                            obj.into()
                        } else {
                            NIL
                        }
                    })
                }) {
                    return result;
                }
                NIL
            },
        );

        // str.match_all(pat) -> array of {value, index, captures}
        // pat must be a regex
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(match_all),
            script_args_def!(pat = NIL),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                let pat = script_value!(vm, args.pat);

                if let Some(re_ptr) = pat.as_regex() {
                    let num_captures = if let Some(re) = vm.bx.heap.regex(re_ptr) {
                        re.num_captures
                    } else {
                        return NIL;
                    };
                    let result = vm.bx.heap.string_mut_self_with(sself, |heap, s| {
                        regex_match_all_detail(heap, s, re_ptr, num_captures)
                    });
                    if let Some(arr) = result {
                        return arr.into();
                    }
                }
                script_err_type_mismatch!(
                    vm.bx.threads.cur_ref().trap,
                    "match_all requires a regex argument"
                )
            },
        );

        // str.replace(pat, replacement) -> string
        // pat can be a string or regex
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(replace),
            script_args_def!(pat = NIL, rep = NIL),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                let pat = script_value!(vm, args.pat);
                let rep = script_value!(vm, args.rep);

                // Regex path
                if let Some(re_ptr) = pat.as_regex() {
                    // Get the replacement string
                    let rep_str = if let Some(r) = vm.bx.heap.string_with(rep, |_, s| s.to_string())
                    {
                        r
                    } else {
                        return script_err_type_mismatch!(
                            vm.bx.threads.cur_ref().trap,
                            "replace: replacement must be a string"
                        );
                    };

                    let result = vm.bx.heap.string_mut_self_with(sself, |heap, s| {
                        regex_replace(heap, s, re_ptr, &rep_str)
                    });
                    if let Some(val) = result {
                        return val;
                    }
                    return script_err_unexpected!(
                        vm.bx.threads.cur_ref().trap,
                        "replace: self must be a string"
                    );
                }

                // String path: replace first occurrence
                if let Some(Some(result)) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    heap.string_mut_self_with(pat, |heap, pat| {
                        heap.string_mut_self_with(rep, |heap, rep| {
                            heap.new_string_from_str(&sself.replacen(pat, rep, 1))
                        })
                    })
                }) {
                    if let Some(result) = result {
                        return result;
                    }
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "replace requires string arguments"
                )
            },
        );

        // str.url_decode() -> string (percent-decode a URL-encoded string)
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(url_decode),
            script_args_def!(),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                if let Some(s) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    let decoded = percent_decode(sself);
                    heap.new_string_from_str(&decoded)
                }) {
                    return s.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "url_decode called on non-string value"
                )
            },
        );

        // str.url_encode() -> string (percent-encode a string for use in URLs)
        native.add_type_method(
            heap,
            ScriptValueType::REDUX_STRING,
            id!(url_encode),
            script_args_def!(),
            |vm, args| {
                let sself = script_value!(vm, args.self);
                if let Some(s) = vm.bx.heap.string_mut_self_with(sself, |heap, sself| {
                    let encoded = percent_encode(sself);
                    heap.new_string_from_str(&encoded)
                }) {
                    return s.into();
                }
                script_err_unexpected!(
                    vm.bx.threads.cur_ref().trap,
                    "url_encode called on non-string value"
                )
            },
        );
    }
}

fn percent_decode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push((hi << 4 | lo) as char);
                i += 3;
                continue;
            }
        } else if bytes[i] == b'+' {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(char::from(b"0123456789ABCDEF"[(b >> 4) as usize]));
                out.push(char::from(b"0123456789ABCDEF"[(b & 0xf) as usize]));
            }
        }
    }
    out
}

// ---- Regex helper functions ----

/// Find all non-overlapping matches and return their byte ranges
fn regex_find_all(
    heap: &ScriptHeap,
    input: &str,
    re_ptr: ScriptRegex,
    num_captures: usize,
) -> Vec<(usize, usize, Vec<Option<(usize, usize)>>)> {
    let mut results = Vec::new();
    let num_slots = (num_captures + 1) * 2;
    let mut slots = vec![None; num_slots];
    let mut search_from = 0;

    while search_from <= input.len() {
        let haystack = &input[search_from..];
        for s in slots.iter_mut() {
            *s = None;
        }

        let re = match heap.regex(re_ptr) {
            Some(re) => re,
            None => break,
        };

        if !re.inner.run(haystack, &mut slots) {
            break;
        }

        let match_start = search_from + slots[0].unwrap_or(0);
        let match_end = search_from + slots[1].unwrap_or(0);

        let mut caps = Vec::new();
        for i in 1..=num_captures {
            let s = slots.get(i * 2).copied().flatten();
            let e = slots.get(i * 2 + 1).copied().flatten();
            match (s, e) {
                (Some(s), Some(e)) => caps.push(Some((search_from + s, search_from + e))),
                _ => caps.push(None),
            }
        }

        results.push((match_start, match_end, caps));

        // Advance past this match (avoid infinite loop on zero-length match)
        if match_end == search_from {
            search_from = next_char_boundary(input, match_end);
        } else {
            search_from = match_end;
        }
    }
    results
}

fn next_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos + 1;
    while p < s.len() && !s.is_char_boundary(p) {
        p += 1;
    }
    p
}

/// Split a string by regex matches
fn regex_split(heap: &mut ScriptHeap, input: &str, re_ptr: ScriptRegex) -> ScriptArray {
    let num_captures = heap.regex(re_ptr).map(|r| r.num_captures).unwrap_or(0);
    let matches = regex_find_all(heap, input, re_ptr, num_captures);

    let array = heap.new_array();
    heap.array_mut_mut_self_with(array, |heap, storage| {
        *storage = ScriptArrayStorage::ScriptValue(Default::default());
        if let ScriptArrayStorage::ScriptValue(vec) = storage {
            let mut last_end = 0;
            for (start, end, caps) in &matches {
                vec.push_back(heap.new_string_from_str(&input[last_end..*start]));
                // JS behavior: include capture groups in split result
                for cap in caps {
                    match cap {
                        Some((s, e)) => vec.push_back(heap.new_string_from_str(&input[*s..*e])),
                        None => vec.push_back(NIL),
                    }
                }
                last_end = *end;
            }
            vec.push_back(heap.new_string_from_str(&input[last_end..]));
        }
    });
    array
}

/// Return array of all matched strings (for global match)
fn regex_match_all_strings(heap: &mut ScriptHeap, input: &str, re_ptr: ScriptRegex) -> ScriptArray {
    let num_captures = heap.regex(re_ptr).map(|r| r.num_captures).unwrap_or(0);
    let matches = regex_find_all(heap, input, re_ptr, num_captures);

    let array = heap.new_array();
    heap.array_mut_mut_self_with(array, |heap, storage| {
        *storage = ScriptArrayStorage::ScriptValue(Default::default());
        if let ScriptArrayStorage::ScriptValue(vec) = storage {
            for (start, end, _) in &matches {
                vec.push_back(heap.new_string_from_str(&input[*start..*end]));
            }
        }
    });
    array
}

/// Execute regex and return first match as a script object
fn regex_exec_first(
    heap: &mut ScriptHeap,
    input: &str,
    re_ptr: ScriptRegex,
    num_captures: usize,
) -> ScriptValue {
    let num_slots = (num_captures + 1) * 2;
    let mut slots = vec![None; num_slots];

    let matched = if let Some(re) = heap.regex(re_ptr) {
        re.inner.run(input, &mut slots)
    } else {
        return NIL;
    };

    if !matched {
        return NIL;
    }

    let match_start = slots[0].unwrap_or(0);
    let match_end = slots[1].unwrap_or(0);
    let value = &input[match_start..match_end];

    let obj = heap.new_with_proto(NIL);
    let value_sv = heap.new_string_from_str(value);
    heap.set_value_def(obj, id!(value).into(), value_sv);
    heap.set_value_def(
        obj,
        id!(index).into(),
        ScriptValue::from_f64(match_start as f64),
    );

    // Build captures array
    let captures = heap.new_array();
    heap.array_mut_mut_self_with(captures, |heap, storage| {
        *storage = ScriptArrayStorage::ScriptValue(Default::default());
        if let ScriptArrayStorage::ScriptValue(vec) = storage {
            vec.push_back(heap.new_string_from_str(value));
            for i in 1..=num_captures {
                let s = slots.get(i * 2).copied().flatten();
                let e = slots.get(i * 2 + 1).copied().flatten();
                match (s, e) {
                    (Some(s), Some(e)) => vec.push_back(heap.new_string_from_str(&input[s..e])),
                    _ => vec.push_back(NIL),
                }
            }
        }
    });
    heap.set_value_def(obj, id!(captures).into(), captures.into());
    obj.into()
}

/// Return array of all match detail objects
fn regex_match_all_detail(
    heap: &mut ScriptHeap,
    input: &str,
    re_ptr: ScriptRegex,
    num_captures: usize,
) -> ScriptArray {
    let matches = regex_find_all(heap, input, re_ptr, num_captures);

    let array = heap.new_array();
    heap.array_mut_mut_self_with(array, |heap, storage| {
        *storage = ScriptArrayStorage::ScriptValue(Default::default());
        if let ScriptArrayStorage::ScriptValue(vec) = storage {
            for (start, end, caps) in &matches {
                let value = &input[*start..*end];
                let obj = heap.new_with_proto(NIL);
                let value_sv = heap.new_string_from_str(value);
                heap.set_value_def(obj, id!(value).into(), value_sv);
                heap.set_value_def(obj, id!(index).into(), ScriptValue::from_f64(*start as f64));

                let cap_arr = heap.new_array();
                heap.array_mut_mut_self_with(cap_arr, |heap, cap_storage| {
                    *cap_storage = ScriptArrayStorage::ScriptValue(Default::default());
                    if let ScriptArrayStorage::ScriptValue(cap_vec) = cap_storage {
                        cap_vec.push_back(heap.new_string_from_str(value));
                        for cap in caps {
                            match cap {
                                Some((s, e)) => {
                                    cap_vec.push_back(heap.new_string_from_str(&input[*s..*e]))
                                }
                                None => cap_vec.push_back(NIL),
                            }
                        }
                    }
                });
                heap.set_value_def(obj, id!(captures).into(), cap_arr.into());
                vec.push_back(obj.into());
            }
        }
    });
    array
}

/// Replace regex matches with a replacement string
/// Supports $& (whole match), $1-$9 (capture groups), $$ (literal $)
fn regex_replace(
    heap: &mut ScriptHeap,
    input: &str,
    re_ptr: ScriptRegex,
    replacement: &str,
) -> ScriptValue {
    let is_global = heap.regex(re_ptr).map(|r| r.flags.global).unwrap_or(false);
    let num_captures = heap.regex(re_ptr).map(|r| r.num_captures).unwrap_or(0);

    let matches = if is_global {
        regex_find_all(heap, input, re_ptr, num_captures)
    } else {
        // Just get the first match
        let all = regex_find_all(heap, input, re_ptr, num_captures);
        if all.is_empty() {
            vec![]
        } else {
            vec![all.into_iter().next().unwrap()]
        }
    };

    if matches.is_empty() {
        return heap.new_string_from_str(input);
    }

    heap.new_string_with(|_heap, out| {
        let mut last_end = 0;
        for (start, end, caps) in &matches {
            out.push_str(&input[last_end..*start]);
            expand_replacement(out, replacement, &input[*start..*end], input, caps);
            last_end = *end;
        }
        out.push_str(&input[last_end..]);
    })
}

/// Expand replacement string patterns ($&, $1, $$, etc.)
fn expand_replacement(
    out: &mut String,
    replacement: &str,
    matched: &str,
    input: &str,
    caps: &[Option<(usize, usize)>],
) {
    let bytes = replacement.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'$' => {
                    out.push('$');
                    i += 2;
                }
                b'&' => {
                    out.push_str(matched);
                    i += 2;
                }
                b'0'..=b'9' => {
                    // Parse group number
                    let start = i + 1;
                    let mut end = start + 1;
                    while end < bytes.len() && bytes[end].is_ascii_digit() {
                        end += 1;
                    }
                    let num: usize = replacement[start..end].parse().unwrap_or(0);
                    if num >= 1 && num <= caps.len() {
                        if let Some((s, e)) = caps[num - 1] {
                            out.push_str(&input[s..e]);
                        }
                    }
                    i = end;
                }
                _ => {
                    out.push('$');
                    i += 1;
                }
            }
        } else {
            let ch = replacement[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
}
