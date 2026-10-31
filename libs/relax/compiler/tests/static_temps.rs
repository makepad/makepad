// static initializers: borrowed temporaries (also &mut) get static storage
static mut RAW: *mut i32 = &mut [42, 43] as *mut [i32] as *mut i32;
static R: &[u32] = &[1, 2, 3];
const C: &(u8, u8) = &(4, 5);
#[test]
fn t() { unsafe { assert_eq!(*RAW, 42); *RAW = 7; assert_eq!(*RAW.add(1), 43); assert_eq!(*RAW, 7); } assert_eq!(R[2], 3); assert_eq!(C.1, 5); }
