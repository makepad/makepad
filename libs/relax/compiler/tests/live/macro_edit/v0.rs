// 7.2: a macro_rules! edit must re-expand every use site: the three fns using `scale!` change
// although their own text does not.
macro_rules! scale {
    ($x:expr) => {
        $x * 2
    };
}

fn one(x: u64) -> u64 {
    scale!(x) + 1
}

fn two(x: u64) -> u64 {
    scale!(x + 1)
}

fn three(x: u64) -> u64 {
    scale!(scale!(x))
}

#[test]
fn run() {
    println!("out {} {} {}", one(5), two(5), three(5));
}
