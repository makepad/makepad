// relax issue 116: an enum's discriminants may cast earlier variants of the same enum, as a flags
// derive writes them (`B = next_bit(E::A as u128) as u64`). The cast of a fieldless variant's path
// is its discriminant constant (rustc mirror_expr_cast), so the enum's layout, not known while its
// discriminants are evaluated, is never needed.
pub const fn next_bit(x: u128) -> u128 {
    1 << x.trailing_ones()
}

const fn neg(x: i64) -> i64 {
    -x
}

// each discriminant after the first casts the ones before it, which cast theirs
#[repr(u64)]
#[derive(Clone, Copy, Debug)]
enum State {
    Invalid = 1,
    Active = next_bit(State::Invalid as u128) as u64,
    Armed = next_bit(State::Invalid as u128 | State::Active as u128) as u64,
}

// a const-fn discriminant without a variant cast before ones with
#[repr(u64)]
#[derive(Clone, Copy, Debug)]
enum Flags {
    A = 1,
    B = next_bit(1u128) as u64,
    C = next_bit(Flags::A as u128 | Flags::B as u128) as u64,
    D = next_bit(Flags::A as u128 | 6u128) as u64,
}

#[repr(i8)]
#[derive(Clone, Copy, Debug)]
enum Signed {
    Low = -3,
    // an offset from `Low`
    Next,
    High = neg(Signed::Next as i64) as i8,
}

// a two-word repr, cast to one word and to two
#[repr(i128)]
#[derive(Clone, Copy, Debug)]
enum Wide {
    Neg = -1,
    Zero,
    Five = Wide::Zero as i128 + 5,
}

fn main() {
    let value = |s: State| s as u128;
    println!(
        "{:?} {:?} {:?} {:?} {}",
        [State::Invalid as u64, State::Active as u64, State::Armed as u64],
        [Flags::A as u64, Flags::B as u64, Flags::C as u64, Flags::D as u64],
        (Signed::Low as i8, Signed::Next as i8, Signed::High as i8, Signed::Next as i128, Signed::Next as u128 == u128::MAX - 1),
        (Wide::Neg as i64, Wide::Zero as u8, Wide::Five as i128, Wide::Neg as u128 == u128::MAX),
        value(State::Armed),
    );
}
