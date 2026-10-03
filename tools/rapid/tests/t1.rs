fn add(a: i64, b: i64) -> i64 {
    a + b
}

fn fib(n: u32) -> u64 {
    if n < 2 {
        return n as u64;
    }
    fib(n - 1) + fib(n - 2)
}

fn sum_to(n: u32) -> u64 {
    let mut s = 0u64;
    for i in 0..n {
        s += i as u64;
    }
    s
}

const K: f64 = 1.5;

fn pair(x: f64) -> (i64, f64) {
    ((x * K) as i64, x - 1.0)
}

#[test]
fn basics() {
    assert_eq!(add(2, 3), 5);
    assert_eq!(fib(20), 6765);
    assert_eq!(sum_to(100), 4950);
    let (a, b) = pair(4.0);
    assert_eq!(a, 6);
    assert!(b == 3.0);
    println!("fib(30) = {}, pi ~ {:.3}, hex {:016x}", fib(30), core::f64::consts::PI, 255u64);
}

#[test]
fn fails() {
    let v = [1, 2, 3];
    let i = add(1, 2) as usize;
    assert_eq!(v[i], 4);
}
