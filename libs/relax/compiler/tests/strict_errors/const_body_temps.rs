//~ none
struct S { value: u32 }
struct SliceHolder { values: &'static [u32] }
struct Holder { value: &'static S }

const fn f() -> u32 { 7 }
const fn slice(values: &'static [u32]) -> SliceHolder { SliceHolder { values } }
const fn hold(value: &'static S) -> Holder { Holder { value } }

static STATIC: SliceHolder = slice(&[f()]);
const CONST: Holder = hold(&S { value: f() });
const INLINE: SliceHolder = const { slice(&[7]) };
const INLINE_REF: &S = const { &S { value: f() } };
impl S {
    const ASSOC: Holder = hold(&S { value: f() });
}

fn main() {
    let _ = (STATIC.values[0], CONST.value.value, INLINE.values[0], INLINE_REF.value, S::ASSOC.value.value);
}
