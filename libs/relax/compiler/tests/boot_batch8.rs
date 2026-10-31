// Boot batch 8 (counter from the AppKit loop to a first frame and a working click, pixel-identical
// to the rustc build): `&str == String` through PartialEq, integer assoc consts and `size_of`/
// `align_of` in array lengths (SILENT: such arrays had length 0 -- draw pass/list uniforms were
// uploaded empty and every frame after the first showed stale pixels), Arc/Box new_uninit /
// assume_init, `Box<[T]>` -> `Box<[T; N]>`, literals selected through TryInto, alias generic
// defaults, `#[rustc_legacy_const_generics]` (stdarch). rustc: all pass.
use std::convert::TryInto;
use std::sync::Arc;

#[test]
fn str_against_string() {
    let s: Arc<str> = Arc::from("ab");
    let svg = String::from("ab");
    assert!(!Some(&s).is_none_or(|s| s.as_ref() != svg));
    let r: &str = "ab";
    assert!(r == svg && svg == r && r != "b");
}

#[repr(C)]
#[derive(Default)]
pub struct Uni {
    a: [f32; 4],
    b: f32,
    c: [f32; 3],
}
impl Uni {
    pub fn as_slice(&self) -> &[f32; std::mem::size_of::<Uni>() >> 2] {
        unsafe { std::mem::transmute(self) }
    }
}

struct Pool<P> {
    classes: [Vec<P>; usize::BITS as usize],
}

#[test]
fn const_lengths_from_layouts_and_assoc_consts() {
    let mut u = Uni::default();
    u.b = 5.0;
    assert!(u.as_slice().len() == 8 && u.as_slice()[4] == 5.0);
    let mut p: Pool<u32> = Pool { classes: std::array::from_fn(|_| Vec::new()) };
    p.classes[8].push(3);
    assert!(p.classes.len() == 64 && p.classes[8].pop() == Some(3));
    let a = [0u8; u8::MAX as usize + 1];
    let b = [0u16; std::mem::align_of::<u64>()];
    assert!(a.len() == 256 && b.len() == 8 && i16::MIN as i32 == -32768);
}

struct Prepared<T>(Box<std::mem::MaybeUninit<T>>);
impl<T> Prepared<T> {
    fn init(mut self, v: T) -> Box<T> {
        self.0.write(v);
        unsafe { self.0.assume_init() }
    }
}

const N: usize = 6;
struct Bufs {
    dict: Box<[u8; N]>,
    next: Box<[u16; 4]>,
}

#[test]
fn uninit_boxes_and_array_boxes() {
    let b = Prepared(Box::new_uninit()).init((5u64, "x".to_string()));
    assert!(b.0 == 5 && b.1 == "x");
    let mut arc = Arc::<[u8]>::new_uninit_slice(4);
    let p = Arc::get_mut(&mut arc).unwrap().as_mut_ptr().cast::<u8>();
    unsafe {
        for i in 0..4 {
            *p.add(i) = i as u8 * 3;
        }
    }
    let arc: Arc<[u8]> = unsafe { arc.assume_init() };
    assert!(&arc[..] == &[0u8, 3, 6, 9][..]);
    let h = Bufs { dict: vec![0; N].into_boxed_slice().try_into().unwrap(), next: vec![0; 4].into_boxed_slice().try_into().unwrap() };
    assert!(h.dict[5] == 0 && h.next[3] == 0);
    let e: Result<Box<[u16; 3]>, _> = vec![1u16; 4].into_boxed_slice().try_into();
    assert!(e.is_err());
}

#[derive(Debug)]
pub struct Error {}
type Result<T, E = Error> = core::result::Result<T, E>;
fn write(src: &[u8], dst: &mut [u8], pos: &mut usize) -> Result<()> {
    match dst.get_mut(*pos..*pos + src.len()) {
        Some(s) => s.copy_from_slice(src),
        None => return Err(Error {}),
    }
    *pos += src.len();
    Ok(())
}

#[test]
fn alias_defaults() {
    let mut d = [0u8; 4];
    let mut p = 0;
    assert!(write(&[1, 2], &mut d, &mut p).is_ok() && write(&[1, 2, 3], &mut d, &mut p).is_err());
}

#[cfg(target_arch = "aarch64")]
#[test]
fn legacy_const_generics() {
    use std::arch::aarch64::*;
    unsafe {
        let v = vdupq_n_u32(3);
        let s = vqshlq_n_u32(v, 6);
        assert!(vgetq_lane_u32(s, 2) == 192);
        let w = vsetq_lane_u32(7, s, 1);
        assert!(vgetq_lane_u32(w, 1) == 7 && vgetq_lane_u32(vshlq_n_u32(w, 1), 1) == 14);
    }
}
