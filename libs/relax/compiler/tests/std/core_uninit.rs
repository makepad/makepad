// std-core differential test (core_diff.sh): Box/Rc/Arc new_uninit(_slice)/new_zeroed(_slice)/assume_init
// (metal.rs: Arc::<[u8]>::new_uninit_slice + Arc::get_mut + assume_init).
use std::mem::MaybeUninit;
use std::rc::Rc;
use std::sync::Arc;

#[test]
fn a01_arc_slice() {
    let mut bgra = Arc::<[u8]>::new_uninit_slice(16);
    let pixels = Arc::get_mut(&mut bgra).unwrap().as_mut_ptr().cast::<u8>();
    for i in 0..16 {
        unsafe { *pixels.add(i) = (i * 3) as u8 };
    }
    let bgra: Arc<[u8]> = unsafe { bgra.assume_init() };
    let c = bgra.clone();
    println!("| arc {} {} {} {}", bgra.len(), bgra[5], c[15], Arc::strong_count(&bgra));
    let z = unsafe { Arc::<[u32]>::new_zeroed_slice(5).assume_init() };
    println!("| arc zeroed {:?}", z);
}

#[test]
fn a02_rc_and_box() {
    let mut r = Rc::<u64>::new_uninit();
    Rc::get_mut(&mut r).unwrap().write(77);
    let r = unsafe { r.assume_init() };
    println!("| rc {} {}", *r, Rc::strong_count(&r));
    let zs = unsafe { Rc::<[u16]>::new_zeroed_slice(3).assume_init() };
    println!("| rc zeroed {:?}", zs);
    let mut s = Rc::<[String]>::new_uninit_slice(2);
    let d = Rc::get_mut(&mut s).unwrap();
    d[0].write(String::from("a"));
    d[1].write(String::from("bc"));
    let s = unsafe { s.assume_init() };
    println!("| rc strings {:?}", s);
    let b = Box::<u32>::new_zeroed();
    println!("| box zeroed {}", unsafe { b.assume_init() });
    let mut bs = Box::<[u8]>::new_uninit_slice(3);
    for (i, x) in bs.iter_mut().enumerate() {
        x.write(i as u8 + 1);
    }
    println!("| box slice {:?}", unsafe { bs.assume_init() });
    println!("| box zeroed slice {:?}", unsafe { Box::<[i8]>::new_zeroed_slice(4).assume_init() });
    let bw = Box::write(Box::<MaybeUninit<f32>>::new(MaybeUninit::uninit()), 2.5);
    println!("| box write {}", bw);
    let a = unsafe { Arc::<u8>::new_zeroed().assume_init() };
    println!("| arc zeroed one {}", a);
}
