// <[[T; N]]>::as_flattened / as_flattened_mut and Vec<[T; N]>::into_flattened (real core/alloc).
#[test]
fn a01_flatten() {
    let quads: Vec<[u8; 4]> = (0..5u8).map(|i| [i, i + 10, i + 20, i + 30]).collect();
    println!("| flat {:?}", quads.as_flattened());
    let mut m = [[1.0f32, 2.0, 3.0, 4.0], [5.0, 6.0, 7.0, 8.0]];
    for x in m.as_flattened_mut().iter_mut() {
        *x *= 2.0;
    }
    println!("| mut {:?}", m);
    let s: &mut [[u16; 4]] = &mut m.map(|r| r.map(|x| x as u16))[..];
    s.as_flattened_mut()[5] = 99;
    println!("| slice mut {:?} len {}", s, s.as_flattened().len());
    let empty: [[u32; 3]; 0] = [];
    println!("| empty {}", empty.as_flattened().len());
    let zst = [[(); 3]; 4];
    println!("| zst {}", zst.as_flattened().len());
    let v: Vec<[i32; 2]> = vec![[1, 2], [3, 4], [5, 6]];
    let cap = v.capacity();
    let f = v.into_flattened();
    println!("| into {:?} cap {}", f, f.capacity() == cap * 2);
    let strings: Vec<[String; 2]> = vec![[String::from("a"), String::from("b")], [String::from("c"), String::from("d")]];
    println!("| strings {:?}", strings.into_flattened());
}
