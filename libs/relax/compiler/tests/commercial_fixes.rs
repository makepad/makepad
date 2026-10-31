// Fixes found running makepad-app-amp under Relax; each assertion is rustc's behaviour.
use std::ops::Shl;

// derive(Clone) with derive(Copy) on a generic type clones field by field (rustc deriving/clone.rs
// is_simple needs no type params): Option<(T, Vec<_>)>::clone must not share the Vec.
#[derive(Clone, Debug, PartialEq)]
enum T { F, I, Vec(u8), Array(Box<T>, u32) }

#[test]
fn derived_clone_generic_copy() {
    let a: Option<(u8, Vec<u32>)> = Some((1, vec![7]));
    let b = a.clone();
    assert_ne!(a.as_ref().unwrap().1.as_ptr(), b.as_ref().unwrap().1.as_ptr());
    let c: Option<(T, Vec<u32>)> = Some((T::Array(Box::new(T::I), 4), vec![1, 2]));
    let mut d = c.clone();
    d.as_mut().unwrap().1.push(3);
    assert_eq!(format!("{:?} {:?}", c, d), "Some((Array(I, 4), [1, 2])) Some((Array(I, 4), [1, 2, 3]))");
    let e = Some(T::Vec(3)).clone();
    assert_eq!(e, Some(T::Vec(3)));
    assert_eq!(T::F.clone(), T::F);
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct W(u32);
impl Shl<u8> for W {
    type Output = W;
    fn shl(self, k: u8) -> W {
        W(self.0 << k)
    }
}

// `&x >> k` has the primitive's type (core's forward-ref Shl/Shr impls); an ADT lhs calls Shl.
#[test]
fn shifts_on_refs() {
    let x: u32 = 12;
    let k: i32 = 2;
    let r = x.checked_shl(k as u32).filter(|y| y >> k == x && *y <= i32::MAX as u32);
    assert_eq!(r, Some(48));
    let y = &48u32;
    let s: u32 = y >> k;
    assert_eq!((s, y << 1u8), (12, 96));
    assert_eq!(W(3) << 2u8, W(12));
}

// The receiver's projections are normalized before probing: `list.last()` on
// `&mut <usize as SliceIndex<[Vec<?T>]>>::Output` is the slice method, not `Iterator for &mut I`.
#[test]
fn method_on_open_projection() {
    let mut held = vec![Vec::new(); 3];
    for (at, id) in [0u32, 1, 1, 2].iter().enumerate() {
        if let Some(list) = held.get_mut(*id as usize) {
            if list.last() != Some(&(at as u32)) {
                list.push(at as u32);
            }
        }
    }
    assert_eq!(held, vec![vec![0], vec![1, 2], vec![3]]);
}

// A format string may be a macro that expands to a string literal (rustc format_args expands
// it eagerly): libs/gltf writer.rs `format!(concat!(..), ..)`.
#[test]
fn format_string_from_macro() {
    assert_eq!(format!(concat!("{{a:", "{}", ",b:{}}}"), 1, "x"), "{a:1,b:x}");
    assert_eq!(format!(concat!("n=", "{n}"), n = 5), "n=5");
    assert_eq!(format!(stringify!(x{}y), 7), "x7y");
}
