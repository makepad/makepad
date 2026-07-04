// Port of box3d/test/test_container.c
// The C b3Array maps to Vec (see container.rs); these tests exercise the same
// operations through the Vec equivalents, including array_remove_swap.

use makepad_box3d::container::array_remove_swap;
use makepad_box3d::ensure;

#[derive(Clone, Copy, Default)]
struct Foo {
    a: i32,
    b: f32,
}

struct Bar {
    a: Vec<i32>,
}

#[test]
fn test_create_destroy() {
    let a: Vec<i32> = Vec::new();
    drop(a);
}

#[test]
fn test_access() {
    let mut a: Vec<i32> = Vec::new();
    a.push(42);
    let element = &a[0];
    ensure!(*element == 42);
}

#[test]
fn test_iteration() {
    let mut a: Vec<i32> = Vec::new();
    a.push(1);
    a.push(2);
    a.push(3);

    let mut sum = 0;
    for i in 0..a.len() {
        sum += a[i];
    }

    ensure!(sum == 6);
}

#[test]
fn test_array_of_struct() {
    let mut a: Vec<Foo> = Vec::new();
    a.push(Foo { a: 1, b: 5.0 });
    a.push(Foo { a: 2, b: 6.0 });
    a.push(Foo { a: 3, b: 7.0 });

    let mut sum1 = 0;
    let mut sum2 = 0.0;
    for i in 0..a.len() {
        sum1 += a[i].a;
        sum2 += a[i].b;
    }

    ensure!(sum1 == 6);
    ensure!(sum2 == 18.0);
}

#[test]
fn test_struct_with_array() {
    let mut a = Bar { a: Vec::new() };
    a.a.push(1);
    a.a.push(2);
    a.a.push(3);

    let mut sum1 = 0;
    for i in 0..a.a.len() {
        sum1 += a.a[i];
    }

    ensure!(sum1 == 6);
}

#[test]
fn test_array_emplace() {
    let mut a: Vec<i32> = Vec::new();

    let n = 100;
    for i in 0..n {
        // b3Array_Emplace: push default, write through the new slot
        a.push(Default::default());
        let last = a.len() - 1;
        a[last] = i;
    }

    let mut sum = 0;
    for i in 0..a.len() {
        sum += a[i];
    }

    ensure!(sum == n * (n - 1) / 2);
}

#[test]
fn test_array_remove() {
    let mut a: Vec<i16> = Vec::new();

    let n = 100;
    a.reserve(n as usize);
    ensure!(a.capacity() >= n as usize && a.is_empty());

    for i in 0..n {
        a.push(i as i16);
    }

    let mut sum: i32 = 0;
    for _ in 0..n {
        let value = a[0];
        sum += value as i32;
        array_remove_swap(&mut a, 0);
    }

    ensure!(sum == (n as i32) * (n as i32 - 1) / 2);
}

#[test]
fn test_array_pop() {
    let mut a: Vec<i32> = Vec::new();

    let n = 10;
    a.resize(n as usize, 0);
    ensure!(a.len() == n as usize);

    for i in 0..n {
        a[i as usize] = i;
    }

    let mut sum = 0;
    while !a.is_empty() {
        sum += a.pop().unwrap();
    }

    ensure!(sum == n * (n - 1) / 2);
}
