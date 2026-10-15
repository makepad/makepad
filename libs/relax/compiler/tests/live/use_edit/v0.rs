// M13: `use` items edited, added and removed live.
//  v1: a module and a `use` of one of its fns are added; run() calls it.
//  v2: the `use` is edited to import the other fn under the same name (`as`).
//  v3: the `use` is removed while run() still calls the name: refused (the running code stays).
mod helpers {
    pub fn double(x: u64) -> u64 {
        x * 2
    }
}

#[test]
fn run() {
    println!("out {}", helpers::double(21));
}
