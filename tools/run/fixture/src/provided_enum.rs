//! Native enum layout across the provided boundary. Every output line must agree with rustc.
use relax_provided_fixture::{
    pod_build, pod_checksum, pod_drops, pod_field_build, pod_field_checksum, pod_maybe,
    pod_option_take, pod_slice_checksum, pod_take, refused_pod_take, Kind, PodDrop, PodField,
    PodTy, RefusedPodTy, V, Id,
};

// Independent caller-side operations, also used as the callees of native reverse calls.
macro_rules! tagged_option_case {
    ($module:ident, $ty:ty, $seed:ident => $value:expr, $x:ident => $sum:expr) => {
        mod $module {
            use relax_provided_fixture::$module as host;

            fn make(present: bool, $seed: u64) -> Option<$ty> {
                if present { Some($value) } else { None }
            }

            fn checksum(value: &Option<$ty>) -> u64 {
                match value { None => 17, Some($x) => 29u64.wrapping_add($sum) }
            }

            fn exchange(value: Option<$ty>, expected: u64, present: bool, seed: u64) -> Option<$ty> {
                assert_eq!(checksum(&value), expected, "caller by-value input");
                make(present, seed)
            }

            fn by_ref(value: &Option<$ty>, expected: u64) -> u64 {
                let got = checksum(value);
                assert_eq!(got, expected, "caller shared-reference input");
                got
            }

            fn by_mut(value: &mut Option<$ty>, expected: u64, present: bool, seed: u64) -> u64 {
                assert_eq!(checksum(value), expected, "caller mutable-reference input");
                *value = make(present, seed);
                checksum(value)
            }

            fn field(value: host::Holder, expected: u64, present: bool, seed: u64) -> host::Holder {
                assert_eq!(value.guard, 0x1234_5678_9abc_def0, "caller handle guard");
                assert_eq!(checksum(&value.value), expected, "caller handle Option field");
                host::Holder { guard: value.guard, value: make(present, seed) }
            }

            extern "C-unwind" fn callback(data: *mut u8, mode: u32, expected: u64, present: bool, seed: u64) -> u64 {
                // SAFETY: reverse supplies an aligned, live Option slot (modes 0..2) or
                // Holder slot (mode 3), exclusively borrowed until this call returns. A
                // by-value call moves the slot out and writes its result back before return.
                unsafe {
                    match mode {
                        0 => {
                            let slot = data.cast::<Option<$ty>>();
                            let out = exchange(slot.read(), expected, present, seed);
                            let sum = checksum(&out);
                            slot.write(out);
                            sum
                        }
                        1 => by_ref(&*data.cast::<Option<$ty>>(), expected),
                        2 => by_mut(&mut *data.cast::<Option<$ty>>(), expected, present, seed),
                        3 => {
                            let slot = data.cast::<host::Holder>();
                            let out = field(slot.read(), expected, present, seed);
                            let sum = checksum(&out.value);
                            slot.write(out);
                            sum
                        }
                        _ => panic!("unknown Option transport"),
                    }
                }
            }

            pub fn run() {
                for present in [false, true] {
                    let input = make(present, 7);
                    let before = checksum(&input);
                    let expected = checksum(&make(!present, 11));
                    let out = host::exchange(input, before, !present, 11);
                    let value_sum = by_ref(&out, expected);
                    let ref_sum = by_ref(host::by_ref(&input, before), before);
                    let mut value = make(present, 7);
                    let returned = host::by_mut(&mut value, before, !present, 11);
                    let mut_sum = by_ref(returned, expected);
                    // The returned &mut also permits the caller to write bytes rustc reads.
                    by_mut(returned, expected, present, 7);
                    host::by_ref(returned, before);
                    let holder = host::Holder { guard: 0x1234_5678_9abc_def0, value: make(present, 7) };
                    let holder = host::field(holder, before, !present, 11);
                    assert_eq!(holder.guard, 0x1234_5678_9abc_def0, "native preserves handle guard");
                    let field_sum = by_ref(&holder.value, expected);
                    let reverse = host::reverse(callback as extern "C-unwind" fn(*mut u8, u32, u64, bool, u64) -> u64 as usize, present, 7);
                    let reverse_expected = before.wrapping_mul(2).wrapping_add(checksum(&make(!present, 8)).wrapping_mul(8));
                    assert_eq!(reverse, reverse_expected, "caller checks native reverse checksum");
                    println!("tagged {} {} value {} ref {} mut {} field {} reverse {}", stringify!($module), present, value_sum, ref_sum, mut_sum, field_sum, reverse);
                }
            }
        }
    };
}

tagged_option_case!(option_u32, u32, seed => 0xfeed_0000 | seed as u32, x => *x as u64);
tagged_option_case!(option_f32, f32, seed => seed as f32 + 0.25, x => x.to_bits() as u64);
tagged_option_case!(option_u64, u64, seed => (seed << 40) | 0xabcd, x => x.rotate_left(13));
tagged_option_case!(option_u128, u128,
    seed => ((seed as u128) << 100) | ((seed as u128) << 52) | 0xcafe,
    x => ((*x >> 64) as u64).wrapping_mul(31).wrapping_add(*x as u64));
tagged_option_case!(option_v, super::V,
    seed => super::V([seed as f32 + 0.25, seed as f32 + 0.5, seed as f32 + 0.75, seed as f32 + 1.0]),
    x => x.0.iter().enumerate().map(|(i, v)| (2 * i as u64 + 1) * v.to_bits() as u64).sum::<u64>());
tagged_option_case!(option_id, super::Id, seed => super::Id((seed << 44) | 0xbeef), x => x.0.rotate_left(7));

// Keep construction and field reads on the caller's side, independent of the bound ops.
fn build(i: u32) -> PodTy {
    match i {
        0 => PodTy::Void,
        1 => PodTy::Bool,
        2 => PodTy::Vec(Kind::B),
        3 => PodTy::Struct {
            align_of: 8,
            size_of: 64,
            fields: vec![
                PodField { name: 11, ty: PodTy::Vec(Kind::A), token: PodDrop { value: 101 } },
                PodField { name: 13, ty: PodTy::Bool, token: PodDrop { value: 103 } },
            ],
        },
        4 => PodTy::FixedArray {
            align_of: 16,
            size_of: 96,
            len: 3,
            ty: Box::new(PodField { name: 17, ty: PodTy::Vec(Kind::B), token: PodDrop { value: 107 } }),
        },
        5 => PodTy::Split {
            fields: vec![
                PodField { name: 11, ty: PodTy::Vec(Kind::A), token: PodDrop { value: 101 } },
                PodField { name: 13, ty: PodTy::Bool, token: PodDrop { value: 103 } },
            ],
            rest: Box::new(PodField { name: 19, ty: PodTy::Bool, token: PodDrop { value: 109 } }),
        },
        _ => panic!("unknown pod variant"),
    }
}

fn field_checksum(field: &PodField) -> u64 {
    17 * field.name as u64 + 19 * field.token.value as u64 + 23 * checksum(&field.ty)
}

fn checksum(value: &PodTy) -> u64 {
    match value {
        PodTy::Void => 3,
        PodTy::Bool => 5,
        PodTy::Vec(kind) => 7 + match kind { Kind::A => 11, Kind::B => 13 },
        PodTy::Struct { align_of, size_of, fields } => {
            let mut sum = 29 + 31 * *align_of as u64 + 37 * *size_of as u64 + 41 * fields.len() as u64;
            for (i, field) in fields.iter().enumerate() {
                sum += (i as u64 + 1) * 43 * field_checksum(field);
            }
            sum
        }
        PodTy::FixedArray { align_of, size_of, len, ty } => {
            47 + 53 * *align_of as u64 + 59 * *size_of as u64 + 61 * *len as u64 + 67 * field_checksum(ty)
        }
        PodTy::Split { fields, rest } => {
            let mut sum = 71 + 73 * fields.len() as u64 + 83 * field_checksum(rest);
            for (i, field) in fields.iter().enumerate() {
                sum += (i as u64 + 1) * 79 * field_checksum(field);
            }
            sum
        }
    }
}

fn replace(value: &mut PodTy, next: PodTy) {
    *value = next;
}

// Lazy compilation must reach the refusal only for this argument, never on the positive path.
fn refused() {
    let value = RefusedPodTy::Field { value: 37 };
    let sum = refused_pod_take(value);
    assert_eq!(sum, 37, "repr(Rust) native value");
    println!("refused native {}", sum);
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("refused") {
        refused();
        return;
    }

    option_u32::run();
    option_f32::run();
    option_u64::run();
    option_u128::run();
    option_v::run();
    option_id::run();

    println!("sizes enum {} option {}", std::mem::size_of::<PodTy>(), std::mem::size_of::<Option<PodTy>>());
    for i in 0..6 {
        let value = pod_build(i);
        let own = checksum(&value);
        let host = pod_checksum(&value);
        assert_eq!(own, host, "rustc-built variant {}", i);
        println!("rustc-built {} {} {}", i, own, host);
    }
    for i in 0..6 {
        let value = build(i);
        let own = checksum(&value);
        let host = pod_checksum(&value);
        let taken = pod_take(value);
        assert_eq!(own, host, "caller-built variant {}", i);
        assert_eq!(own, taken, "by-value variant {}", i);
        println!("caller-built {} {} {} {}", i, own, host, taken);
    }

    for (i, count) in [(3, 2), (4, 1), (5, 3)] {
        let before = pod_drops();
        drop(pod_build(i));
        let caller = pod_drops() - before;
        assert_eq!(caller, count, "caller drops rustc payload {}", i);
        let before = pod_drops();
        pod_take(build(i));
        let host = pod_drops() - before;
        assert_eq!(host, count, "rustc drops caller payload {}", i);
        println!("drops {} caller {} rustc {}", i, caller, host);
    }

    let none = pod_maybe(0, false);
    assert!(none.is_none(), "rustc returns None");
    let none_sum = pod_option_take(None);
    assert_eq!(none_sum, 0, "rustc takes None");
    println!("option none {} {}", none.is_none(), none_sum);
    for i in 0..6 {
        let before = pod_drops();
        let value = pod_maybe(i, true);
        let own = match value.as_ref() { Some(value) => checksum(value) + 1, None => panic!("expected Some") };
        drop(value);
        let caller_drops = pod_drops() - before;
        let before = pod_drops();
        let host = pod_option_take(Some(build(i)));
        let host_drops = pod_drops() - before;
        assert_eq!(own, host, "Option variant {}", i);
        assert_eq!(caller_drops, host_drops, "Option drops {}", i);
        println!("option some {} {} {} drops {} {}", i, own, host, caller_drops, host_drops);
    }

    let mut values = Vec::new();
    let mut own = 0;
    for i in 0..6 {
        let value = build(i);
        own += (i as u64 + 1) * checksum(&value);
        values.push(value);
    }
    let host = pod_slice_checksum(&values);
    assert_eq!(own, host, "Vec of enums read by rustc");
    println!("vec {} {} {}", values.len(), own, host);
    drop(values);

    let field = pod_field_build(3);
    let own = field_checksum(&field);
    let host = pod_field_checksum(&field);
    assert_eq!(own, host, "rustc-built handle field");
    println!("field rustc-built {} {}", own, host);
    drop(field);
    let field = PodField { name: 23, ty: build(4), token: PodDrop { value: 113 } };
    let own = field_checksum(&field);
    let host = pod_field_checksum(&field);
    assert_eq!(own, host, "caller-built handle field");
    println!("field caller-built {} {}", own, host);
    drop(field);

    let before = pod_drops();
    let mut value = pod_build(3);
    replace(&mut value, build(4));
    let old_drops = pod_drops() - before;
    assert_eq!(old_drops, 2, "assignment drops old Vec payload");
    let own = checksum(&value);
    let host = pod_checksum(&value);
    assert_eq!(own, host, "assignment keeps replacement");
    drop(value);
    let total_drops = pod_drops() - before;
    assert_eq!(total_drops, 3, "assignment drops replacement Box once");
    println!("assignment {} {} drops {} {}", own, host, old_drops, total_drops);

    let mut value = pod_build(3);
    if let PodTy::Struct { align_of, size_of, fields } = &mut value {
        *align_of = 32;
        *size_of = 128;
        fields[0].name = 29;
        fields[0].ty = PodTy::Vec(Kind::B);
        fields[1].token.value = 127;
    } else {
        panic!("expected Struct");
    }
    let own = checksum(&value);
    let host = pod_checksum(&value);
    assert_eq!(own, host, "writes through mutable variant bindings");
    println!("field-write {} {}", own, host);
    drop(value);

    let value = pod_build(3);
    let guarded = match value {
        PodTy::Struct { ref fields, size_of, .. } if fields.len() == 2 && fields[0].name == 11 => {
            size_of as u64 + field_checksum(&fields[1])
        }
        _ => panic!("ref guard did not match"),
    };
    assert_eq!(guarded, 2357, "guard reads borrowed fields");
    assert_eq!(checksum(&value), pod_checksum(&value), "ref guard keeps value usable");
    println!("ref-guard {} {}", guarded, pod_checksum(&value));
    drop(value);

    let before = pod_drops();
    let fields = {
        let value = pod_build(5);
        match value {
            PodTy::Split { fields, .. } => fields,
            _ => panic!("expected Split"),
        }
    };
    let rest_drops = pod_drops() - before;
    assert_eq!(rest_drops, 1, "partial move drops only the remaining Box");
    let moved_sum = field_checksum(&fields[0]) + field_checksum(&fields[1]);
    assert_eq!(moved_sum, 4813, "partial move preserves Vec fields");
    drop(fields);
    let total_drops = pod_drops() - before;
    assert_eq!(total_drops, 3, "partial move drops each token exactly once");
    println!("partial-move {} drops {} {}", moved_sum, rest_drops, total_drops);
}
