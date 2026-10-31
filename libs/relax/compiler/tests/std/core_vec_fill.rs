// vec![x; n] and Vec::resize: Copy fills (memset / doubling copies), zeroed allocations for
// all-zero values, Clone fills with the last element moved in, ZSTs, shrinking.
fn sum_bytes<T>(v: &[T]) -> u64 {
    let p = v.as_ptr() as *const u8;
    let n = std::mem::size_of_val(v);
    let mut h = 0u64;
    for i in 0..n {
        h = h.wrapping_mul(31).wrapping_add(unsafe { *p.add(i) } as u64);
    }
    h
}

#[derive(Clone, Debug, PartialEq)]
struct Counted(u32, String);

#[test]
fn a01_copy_fills() {
    for n in [0usize, 1, 2, 3, 7, 8, 9, 31, 1000] {
        let a = vec![0u8; n];
        let b = vec![0xabu8; n];
        let c = vec![7u32; n];
        let d = vec![0.0f32; n];
        let e = vec![-0.0f64; n];
        let f = vec![(1u16, 2u16); n];
        let g = vec![true; n];
        let h = vec!['z'; n];
        let k = vec![[0u8; 3]; n];
        println!("| n {} {} {} {} {} {} {} {} {} {}", n, sum_bytes(&a), sum_bytes(&b), sum_bytes(&c), sum_bytes(&d), sum_bytes(&e), sum_bytes(&f), g.iter().filter(|x| **x).count(), h.iter().collect::<String>().len(), sum_bytes(&k));
        println!("| caps {} {} {}", a.capacity() >= n, c.len(), e.iter().all(|x| x.is_sign_negative()));
    }
    let big = vec![0u64; 1 << 20];
    println!("| big {} {}", big.len(), big.iter().sum::<u64>());
}

#[test]
fn a02_clone_fills() {
    let v = vec![Counted(3, String::from("ab")); 5];
    println!("| clones {:?}", v);
    let nested = vec![vec![1u8, 2]; 3];
    println!("| nested {:?}", nested);
    let none: Vec<String> = vec![String::from("x"); 0];
    println!("| none {}", none.len());
    let units = vec![(); 1_000_000];
    println!("| zst {}", units.len());
}

#[test]
fn a03_resize() {
    let mut v: Vec<u32> = vec![1, 2, 3];
    v.resize(10, 9);
    println!("| grow {:?}", v);
    v.resize(2, 0);
    println!("| shrink {:?}", v);
    let mut s: Vec<String> = Vec::new();
    s.resize(3, String::from("q"));
    s.resize(4, String::from("r"));
    println!("| strings {:?}", s);
    let mut b: Vec<u8> = Vec::with_capacity(4);
    b.resize(100, 0x5a);
    println!("| bytes {} {}", b.len(), sum_bytes(&b));
    let mut z: Vec<f32> = vec![1.5; 3];
    z.resize(6, 0.0);
    println!("| floats {:?}", z);
}
