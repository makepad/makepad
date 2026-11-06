// The derives of a generic type go while the generic fn using them changes its signature to stop
// using them (v2); v1 first changes that fn's signature alone. A removed derive(PartialEq) takes
// its marker impl with it, which rebuilds the code that inlined the type's drop glue. The fns
// each edit replaced are code in flight: their old text was rebuilt against the new impls, and
// v2 was refused with "no `PartialEq` impl to compare `&W<T0>` with `W<T0>`" at v0's body.
#[derive(Debug)]
pub struct W<T> {
    v: T,
}

fn twice<T>(w: &W<T>) -> bool {
    let _ = w;
    false
}

#[test]
fn run() {
    let w = W { v: 3u32 };
    println!("out {} {} {:?}", twice(&w), w.v, w);
}
