// HotRust closures: captures by reference and by move, Fn-bound inference, impl Fn params,
// calls through references, nested closures, closures in generic instances.

fn apply<F: Fn(i64) -> i64>(f: F, x: i64) -> i64 {
    f(x)
}

fn apply_mut<F>(mut f: F, n: i64)
where
    F: FnMut(i64),
{
    let mut i = 0;
    while i < n {
        f(i);
        i += 1;
    }
}

fn apply_ref(f: &dyn_free::Adder, x: i64) -> i64 {
    f.add(x)
}

mod dyn_free {
    pub struct Adder {
        pub k: i64,
    }
    impl Adder {
        pub fn add(&self, x: i64) -> i64 {
            x + self.k
        }
    }
}

fn twice(f: impl Fn(i64) -> i64, x: i64) -> i64 {
    f(f(x))
}

fn call_by_ref<F: Fn(i64) -> i64>(f: &F, x: i64) -> i64 {
    f(x)
}

fn map_pair<T: Copy, U, F: Fn(T) -> U>(a: T, b: T, f: F) -> (U, U) {
    (f(a), f(b))
}

fn make_adder(k: i64) -> i64 {
    let add = move |x: i64| x + k;
    add(1) + add(2)
}

#[test]
fn captures() {
    let k = 10;
    assert_eq!(apply(|x| x + k, 5), 15);
    let mut sum = 0;
    apply_mut(|i| sum += i, 5);
    assert_eq!(sum, 10);
    let a = dyn_free::Adder { k: 3 };
    assert_eq!(apply_ref(&a, 4), 7);
    assert_eq!(twice(|x| x * k, 2), 200);
    let m = 7;
    let f = |x: i64| x - m;
    assert_eq!(call_by_ref(&f, 10), 3);
    assert_eq!(f(1), -6);
    let (p, q) = map_pair(2, 3, |v: i64| v * v + k);
    assert_eq!(p, 14);
    assert_eq!(q, 19);
    assert_eq!(make_adder(100), 203);
}

#[test]
fn nested_and_mutation() {
    let mut count = 0;
    let mut bump = |n: i64| {
        let mut inner = |m: i64| count += m;
        inner(n);
        inner(1);
    };
    bump(5);
    bump(2);
    assert_eq!(count, 9);
    let base = 1000;
    let g = |x: i64| {
        let h = |y: i64| y + base;
        h(x) * 2
    };
    assert_eq!(g(1), 2002);
}

#[test]
fn fn_items_and_pointers() {
    fn sq(x: i64) -> i64 {
        x * x
    }
    assert_eq!(apply(sq, 9), 81);
    let p: fn(i64) -> i64 = sq;
    assert_eq!(apply(p, 4), 16);
    let c: fn(i64) -> i64 = |x| x + 1;
    assert_eq!(c(1), 2);
}
