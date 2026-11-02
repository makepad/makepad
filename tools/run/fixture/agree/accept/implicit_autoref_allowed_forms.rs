// What dangerous_implicit_autorefs leaves alone: explicit references, unsizing receivers and methods
// without the attribute.
pub struct S { pub v: Vec<u8>, pub a: [u8; 4] }
pub unsafe fn f(p: *mut S, s: *const [u8], a: *const [u8; 4]) -> usize {
    unsafe { (&*s).len() + (*a).len() + (*p).v.capacity() + (*p).a.len() + (*p).a[1] as usize + (&(*p).v).len() }
}
#[test] fn runs() {
    let mut s = S { v: vec![1], a: [0, 2, 0, 0] };
    assert_eq!(unsafe { f(&mut s, &[1u8, 2][..], &[0; 4]) }, 2 + 4 + 1 + 4 + 2 + 1);
}
