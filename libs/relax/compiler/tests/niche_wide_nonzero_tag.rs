// Option<NonZero<u128>> / Option<NonZero<i128>> and an Option of a u128-tagged enum: the 16-byte niche
// keeps them 16 bytes as rustc lays them out, and its tag reads as Some for every valid value, those
// whose low 64 bits are 0 included (the discriminant compares both words of the tag).
use std::hint::black_box;
use std::num::NonZero;

#[repr(u128)]
#[derive(Clone, Copy, Debug, PartialEq)]
enum Wide {
    High = 1 << 64,
    Low = 7,
}

#[test]
fn wide_niches_keep_their_values() {
    for v in [1u128 << 64, 1u128 << 127, u128::MAX, 1] {
        let o = NonZero::new(black_box(v));
        assert_eq!(o.map(NonZero::get), Some(v), "{v:#x}");
        assert!(matches!(o, Some(n) if n.get() == v), "{v:#x} in a match");
    }
    assert!(NonZero::new(black_box(i128::MIN)).is_some(), "i128::MIN");
    assert_eq!(NonZero::new(black_box(0u128)), None);
    assert_eq!(std::mem::size_of::<Option<NonZero<u128>>>(), 16);
    let code = |w: Option<Wide>| match w {
        Some(Wide::High) => 1,
        Some(Wide::Low) => 2,
        None => 3,
    };
    assert_eq!([Some(Wide::High), Some(Wide::Low), None].map(|w| code(black_box(w))), [1, 2, 3], "u128 tags, one only the high word tells");
    assert_eq!(std::mem::size_of::<Option<Wide>>(), 16);
}
