// core::intrinsics_mem::is_copy::<T>() (std lane request; Relax-only intrinsic, folded per
// instance): true exactly when T is Copy, never for a Copy-conditional ADT over a non-Copy arg
// (std's memcpy paths for extend_from_slice / to_vec / clone rely on it).
use core::intrinsics_mem::is_copy;

#[derive(Clone, Copy)]
struct P(u8, f32);

#[derive(Clone)]
struct Q(u8);

struct G<T>(T);

impl<T: Copy> Clone for G<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: Copy> Copy for G<T> {}

fn of<T>() -> bool {
    unsafe { is_copy::<T>() }
}

#[test]
fn is_copy_per_instance() {
    assert_eq!(
        [of::<u8>(), of::<P>(), of::<(u8, &str)>(), of::<[P; 3]>(), of::<G<u8>>(), of::<Option<u32>>(), of::<fn()>(), of::<*mut String>()],
        [true; 8]
    );
    assert_eq!([of::<Q>(), of::<String>(), of::<G<String>>(), of::<&mut u8>(), of::<Option<Vec<u8>>>(), of::<[String; 2]>()], [false; 6]);
}
