// A discriminant written in terms of an earlier variant of the same enum (`Self::XStart as u8`):
// stylo's `PositionAreaKeyword` (values/specified/position.rs) builds its self-relative keywords
// that way (`SelfXStart = SELF_WM | (Self::XStart as u8)`). The cast built the variant's value,
// which needs the enum's layout, which needs every discriminant of the enum, the one being
// evaluated included: E0391 (a cycle), and E0080 for each variant written from such a variant.
// Only the `Self::` spelling failed: `Keyword::YStart as u8` is folded from the path alone.

#[repr(u8)]
#[derive(Clone, Copy)]
enum Axis {
    None = 0,
    Horizontal = 1,
    Vertical = 2,
}

#[repr(u8)]
#[derive(Clone, Copy)]
enum Track {
    Start = 1,
    End = 2,
}

const AXIS_SHIFT: usize = 3;
const SELF_WM: u8 = 1 << 6;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keyword {
    None = (Axis::None as u8) << AXIS_SHIFT,
    XStart = ((Axis::Horizontal as u8) << AXIS_SHIFT) | Track::Start as u8,
    XEnd = ((Axis::Horizontal as u8) << AXIS_SHIFT) | Track::End as u8,
    YStart = ((Axis::Vertical as u8) << AXIS_SHIFT) | Track::Start as u8,
    SelfXStart = SELF_WM | (Self::XStart as u8),
    SelfXEnd = SELF_WM | (Self::XEnd as u8),
    SelfYStart = SELF_WM | (Keyword::YStart as u8),
    // through a chain: a variant that is itself written from an earlier one
    Twice = (Self::SelfXEnd as u8) | 0x80,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Plain {
    A = 3,
    B = Self::A as isize * 2,
    C,
}

const fn twice(x: i8) -> i8 {
    let (mut n, mut i) = (0, 0);
    while i < 2 {
        n += x;
        i += 1;
    }
    n
}
const fn pick(xs: &[i8], i: usize) -> i8 {
    xs[i]
}

#[repr(i8)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Signed {
    Neg = -3,
    Twice = twice(Self::Neg as i8),
    Picked = pick(&[Self::Neg as i8, Signed::Twice as i8, 9], 1) + 1,
    // a match is evaluated as compiled code, not by the constant evaluator
    Matched = match Self::Picked as i8 {
        -5 => 50,
        _ => 0,
    },
}

#[test]
fn a_discriminant_reads_an_earlier_variant_of_its_own_enum() {
    assert_eq!(Keyword::None as u8, 0);
    assert_eq!(Keyword::XStart as u8, 0b001_001);
    assert_eq!(Keyword::XEnd as u8, 0b001_010);
    assert_eq!(Keyword::SelfXStart as u8, 0b1_001_001);
    assert_eq!(Keyword::SelfXEnd as u8, 0b1_001_010);
    assert_eq!(Keyword::SelfYStart as u8, 0b1_010_001);
    assert_eq!(Keyword::Twice as u8, 0b11_001_010);
    assert_eq!(core::mem::size_of::<Keyword>(), 1);
    // the values are what a match and a transmute see too
    let k: Keyword = unsafe { core::mem::transmute(0b1_001_010u8) };
    assert_eq!(k, Keyword::SelfXEnd);
    let name = match k {
        Keyword::SelfXEnd => "self-x-end",
        _ => "other",
    };
    assert_eq!(name, "self-x-end");
    assert_eq!((Plain::A as isize, Plain::B as isize, Plain::C as isize), (3, 6, 7));
    assert_eq!((Signed::Neg as i8, Signed::Twice as i8, Signed::Picked as i8, Signed::Matched as i8), (-3, -6, -5, 50));
    // the cast of a variant keeps the sign of the enum's representation at every width
    assert_eq!(Signed::Neg as i128, -3);
    assert_eq!(Signed::Neg as u128, u128::MAX - 2);
    assert_eq!(Signed::Neg as i64, -3);
    assert_eq!(Signed::Neg as u64, u64::MAX - 2);
    assert_eq!(Signed::Neg as u8, 253);
    assert_eq!((Signed::Twice) as isize, -6);
}
