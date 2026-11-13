extern "C" fn inc(x: i32) -> i32 { x + 1 }
fn main() {
    // tests/bin_exe.rs: the name this binary was started under (its argv[0])
    if std::env::args().nth(1).as_deref() == Some("--name") {
        let arg0 = std::env::args().next().unwrap();
        println!("{}", std::path::Path::new(&arg0).file_stem().unwrap().to_string_lossy());
        return;
    }
    println!("add {}", cnative::add(40, 2));
    println!("hash {:016x}", cnative::hash(b"makepad"));
    println!("apply {}", cnative::apply(inc, 20));
    println!("cfg {}", cnative::built());
    println!("offset {}", cnative::offset());
    println!("callback {}", cnative::square_plus(7));
    println!("shadowed {}", cnative::shadowed());
}
