// Widened repr(Rust) tags and their niches: exercise values, not uninitialised padding.
// These layout assertions cannot distinguish a 16-byte tag from an 8-byte tag plus padding.
use std::fmt::Debug;
use std::hint::black_box;
use std::mem::{align_of, size_of};

#[repr(align(16))]
#[derive(Clone, Debug, PartialEq)]
struct V([f32; 4]);

#[repr(align(32))]
#[derive(Clone, Debug, PartialEq)]
struct Aligned(u128);

#[derive(Clone, Debug, PartialEq)]
enum Three {
    A(u128),
    B(u128),
    C(u128),
}

const U: u128 = (1u128 << 100) | 37;
const I: i128 = -(1i128 << 110) + 19;
const UNSIGNED: [Option<u128>; 2] = [None, Some(U)];
const SIGNED: [Option<i128>; 2] = [None, Some(I)];
const NESTED: [Option<Option<u128>>; 3] = [None, Some(None), Some(Some(U))];
const VECTOR: [Option<V>; 2] = [None, Some(V([1.0, -2.0, 3.5, 4.0]))];
const THREE: [Three; 3] = [Three::A(U), Three::B(U + 1), Three::C(U + 2)];
const RESULT: [Result<u128, u64>; 2] = [Ok(U), Err(71)];
const ALIGNED: [Option<Aligned>; 2] = [None, Some(Aligned(U))];

static S_UNSIGNED: [Option<u128>; 2] = UNSIGNED;
static S_SIGNED: [Option<i128>; 2] = SIGNED;
static S_NESTED: [Option<Option<u128>>; 3] = NESTED;
static S_VECTOR: [Option<V>; 2] = VECTOR;
static S_THREE: [Three; 3] = THREE;
static S_RESULT: [Result<u128, u64>; 2] = RESULT;
static S_ALIGNED: [Option<Aligned>; 2] = ALIGNED;

#[inline(never)]
fn by_value<T>(value: T) -> T {
    black_box(value)
}

fn exercise<T: Clone + Debug + PartialEq>(values: &[T], constants: &[T], statics: &[T], size: usize, align: usize, name: &str) {
    assert_eq!(size_of::<T>(), size, "{name}: size");
    assert_eq!(align_of::<T>(), align, "{name}: alignment");
    assert_eq!(values, constants, "{name}: constructed and const values");
    assert_eq!(values, black_box(statics), "{name}: static values");
    let mut stored = Vec::new();
    for value in values {
        let cloned = black_box(value).clone();
        assert_eq!(&cloned, value, "{name}: clone");
        let returned = by_value(cloned);
        assert_eq!(&returned, value, "{name}: return by value");
        match by_value(Some(Some(returned))) {
            Some(Some(inner)) => {
                assert_eq!(&inner, value, "{name}: nested match");
                stored.push(inner);
            }
            _ => panic!("{name}: nested variant"),
        }
    }
    assert_eq!(black_box(stored.as_slice()), values, "{name}: Vec values");
}

fn payload_offset<T, P>(value: &T, payload: &P) -> isize {
    // Both byte pointers refer to the same enum allocation; no padding is read.
    unsafe { (payload as *const P as *const u8).offset_from(value as *const T as *const u8) }
}

fn some_offset<T>(value: &Option<T>) -> isize {
    match value {
        Some(payload) => payload_offset(value, payload),
        None => panic!("payload offset needs Some"),
    }
}

#[test]
fn widened_tags_preserve_values() {
    let unsigned = [None, Some(black_box(U))];
    let signed = [None, Some(black_box(I))];
    let nested = [None, Some(None), Some(Some(black_box(U)))];
    let vector = [None, Some(V(black_box([1.0, -2.0, 3.5, 4.0])))];
    let three = [Three::A(black_box(U)), Three::B(black_box(U + 1)), Three::C(black_box(U + 2))];
    let result = [Ok(black_box(U)), Err(black_box(71))];
    let aligned = [None, Some(Aligned(black_box(U)))];

    exercise(&unsigned, &UNSIGNED, &S_UNSIGNED, 32, 16, "Option<u128>");
    exercise(&signed, &SIGNED, &S_SIGNED, 32, 16, "Option<i128>");
    exercise(&nested, &NESTED, &S_NESTED, 32, 16, "Option<Option<u128>>");
    exercise(&vector, &VECTOR, &S_VECTOR, 32, 16, "Option<V>");
    exercise(&three, &THREE, &S_THREE, 32, 16, "Three");
    // Mixed first-field alignments widen only to 8; no integer matches alignment 32.
    exercise(&result, &RESULT, &S_RESULT, 32, 16, "Result<u128, u64>");
    exercise(&aligned, &ALIGNED, &S_ALIGNED, 64, 32, "Option<Aligned>");

    assert_eq!(some_offset(black_box(&unsigned[1])), 16, "Option<u128>: payload offset");
    assert_eq!(some_offset(black_box(&signed[1])), 16, "Option<i128>: payload offset");
    assert_eq!(some_offset(black_box(&nested[2])), 0, "Option<Option<u128>>: outer payload offset");
    match black_box(&nested[2]) {
        Some(inner) => assert_eq!(some_offset(inner), 16, "Option<Option<u128>>: inner payload offset"),
        None => panic!("Option<Option<u128>>: outer variant"),
    }
    assert_eq!(some_offset(black_box(&vector[1])), 16, "Option<V>: payload offset");
    assert_eq!(some_offset(black_box(&aligned[1])), 32, "Option<Aligned>: payload offset");
    for (index, value) in three.iter().enumerate() {
        let (tag, payload) = match black_box(value) {
            Three::A(payload) => (0, payload),
            Three::B(payload) => (1, payload),
            Three::C(payload) => (2, payload),
        };
        assert_eq!(tag, index, "Three: matched variant");
        assert_eq!(*payload, U + index as u128, "Three: matched payload");
        assert_eq!(payload_offset(value, payload), 16, "Three: payload offset");
    }
    match black_box(&result[0]) {
        Ok(payload) => assert_eq!(payload_offset(&result[0], payload), 16, "Result: Ok payload offset"),
        Err(_) => panic!("Result: Ok variant"),
    }
    match black_box(&result[1]) {
        Err(payload) => assert_eq!(payload_offset(&result[1], payload), 8, "Result: Err payload offset"),
        Ok(_) => panic!("Result: Err variant"),
    }
}
