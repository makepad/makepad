// `&mut [T; N] as *const T` borrows mutably, nothing else: it captures `*this` by unique borrow.
pub fn plain(this: &mut [u32; 2]) -> *const u32 { this as *const u32 }
pub fn captured(this: &mut [u32; 2]) { let mut f = || { let _p = this as *const u32; }; f(); this[0] += 1; }
#[test] fn runs() { let mut a = [1, 2]; assert!(!plain(&mut a).is_null()); captured(&mut a); assert_eq!(a[0], 2); }
