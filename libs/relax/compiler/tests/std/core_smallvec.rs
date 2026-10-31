// std-core differential test (core_diff.sh): Relax's built-in smallvec (std/smallvec),
// mounted with #[path] on both sides until the loader substitutes the crate (coord.md SV1); its
// equivalence with the real smallvec 1.15.1 under rustc is std/check_ext.
#[path = "../../std/smallvec/lib.rs"]
mod smallvec;

use smallvec::SmallVec;

fn show(tag: &str, v: &SmallVec<[u32; 4]>) {
    print!("| {} len {} cap {} spilled {}:", tag, v.len(), v.capacity(), v.spilled());
    for x in v.iter() {
        print!(" {}", x);
    }
    println!();
}

#[test]
fn a01_push_spill_shrink() {
    let mut v: SmallVec<[u32; 4]> = SmallVec::new();
    show("new", &v);
    let mut i = 0;
    while i < 6 {
        v.push(i * 3);
        show("push", &v);
        i += 1;
    }
    v.truncate(3);
    v.shrink_to_fit();
    show("shrunk", &v);
    v.insert(1, 99);
    println!("| removed {}", v.remove(0));
    show("edit", &v);
    v.retain(|x| *x != 99);
    show("retain", &v);
}

#[test]
fn a02_iter_collect() {
    let v: SmallVec<[u32; 4]> = (0..10u32).collect();
    show("collect", &v);
    let mut s = 0;
    for x in v.into_iter() {
        s += x;
    }
    println!("| sum {}", s);
}
