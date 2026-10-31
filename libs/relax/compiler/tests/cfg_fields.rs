// `#[cfg(..)]` on fields (boot B30/B29): cfg'd-out fields are not part of the type, its layout,
// struct literals, struct patterns or derives. rustc: all pass.
#[derive(Default, Debug, Clone, PartialEq)]
struct S {
    a: u32,
    #[cfg(windows)]
    w: NoSuchType,
    #[cfg(not(windows))]
    b: u8,
}

#[repr(C)]
struct C {
    x: u64,
    #[cfg(windows)]
    pad: [u64; 4],
    y: u32,
}

struct T(u32, #[cfg(windows)] NoSuchType, u16);

enum E {
    V {
        a: u8,
        #[cfg(windows)]
        w: NoSuchType,
    },
}

#[test]
fn cfg_fields_default_and_size() {
    let s = S::default();
    assert!(s.a == 0 && s.b == 0);
    assert!(s == S { a: 0, #[cfg(windows)] w: NoSuchType, b: 0 });
    assert!(std::mem::size_of::<S>() == 8);
}

#[test]
fn cfg_fields_repr_c_layout() {
    let c = C { x: 1, #[cfg(windows)] pad: [0; 4], y: 2 };
    assert!(std::mem::size_of::<C>() == 16);
    assert!(c.x + c.y as u64 == 3);
}

#[test]
fn cfg_fields_tuple_and_pattern() {
    let t = T(3, 4);
    let T(a, b) = t;
    let e = E::V { a: 5, #[cfg(windows)] w: NoSuchType };
    let E::V { a: c, #[cfg(windows)] w: _ } = e;
    assert!(a as u16 + b + c as u16 == 12);
}
