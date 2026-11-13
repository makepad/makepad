// std-core differential test (core_diff.sh): arrays by value, Default/Eq/TryFrom.
#[test]
fn a01() {
    let mut s = 0u32;
    for x in [1u32, 2, 3] { s += x; }
    println!("| sum {}", s);
    let v: Vec<u8> = [4u8, 5].into_iter().collect();
    println!("| v {}", v.len());
    let d: [u16; 3] = Default::default();
    println!("| d {}", d[2]);
    println!("| eq {}", [1, 2] == [1, 2]);
    let t: [u8; 2] = (&[9u8, 8][..]).try_into().unwrap();
    println!("| t {}", t[1]);
}
#[test]
fn a02_try_from_assoc() {
    let t: [u8; 2] = <[u8; 2]>::try_from(&[9u8, 8][..]).unwrap();
    println!("| t {}", t[0]);
    let e = <[u8; 3]>::try_from(&[1u8][..]).is_err();
    println!("| err {}", e);
}
#[test]
fn a03_from_fn_const_generic() {
    let a: [u32; 5] = std::array::from_fn(|i| (i * i) as u32);
    println!("| {:?}", a);
    let b = a.map(|x| x + 1);
    println!("| {:?} {}", b, b.len());
    let mut s = 0;
    for x in [1u8, 2, 3] { s += x; }
    println!("| {}", s);
    println!("| {}", "a,b;c".split([',', ';']).count());
    let v: Vec<u16> = Vec::from([5u16, 6]);
    println!("| {:?} {}", v, v == [5u16, 6]);
    let d: [i8; 32] = Default::default();
    println!("| {}", d.len());
}
#[test]
fn a04_vec_into_boxed_array() {
    // relax issue 98: a Vec of N elements is a Box<[T; N]>, any other comes back as it was
    use std::sync::atomic::{AtomicUsize, Ordering};
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    #[derive(Debug)]
    struct D(u8);
    impl Drop for D {
        fn drop(&mut self) {
            // the sum of the dropped values: which ones dropped, each once
            DROPS.fetch_add(self.0 as usize, Ordering::SeqCst);
        }
    }
    let b: Box<[u8; 3]> = vec![1u8, 2, 3].try_into().unwrap();
    println!("| boxed {:?}", b);
    let big: Box<[usize; 288]> = (0..288).collect::<Vec<_>>().try_into().unwrap();
    println!("| big {} {}", big.len(), big.iter().sum::<usize>());
    let mut spare: Vec<u32> = Vec::with_capacity(10);
    spare.extend([7u32, 8]);
    let fit: Box<[u32; 2]> = spare.try_into().unwrap();
    println!("| fit {:?}", fit);
    let mut short: Vec<String> = Vec::with_capacity(8);
    short.extend(["a", "b"].map(String::from));
    let back = <Box<[String; 3]>>::try_from(short).unwrap_err();
    println!("| back {:?} {}", back, back.capacity());
    let zst: Box<[(); 4]> = vec![(); 4].try_into().unwrap();
    let empty: Box<[u8; 0]> = Vec::new().try_into().unwrap();
    println!("| zst {} empty {}", zst.len(), empty.len());
    let owned: Box<[D; 3]> = vec![D(1), D(2), D(3)].try_into().unwrap();
    let after_move = DROPS.load(Ordering::SeqCst);
    println!("| owned {:?} drops {}", owned, after_move);
    drop(owned);
    let refused = <Box<[D; 2]>>::try_from(vec![D(4)]).unwrap_err();
    println!("| drops {} refused {:?}", DROPS.load(Ordering::SeqCst), refused);
    drop(refused);
    println!("| drops {}", DROPS.load(Ordering::SeqCst));
}
