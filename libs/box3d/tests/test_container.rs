// Port of box3d/test/test_container.c
// The C b3Array maps to Vec; array_remove_swap is the port-specific helper.

use makepad_box3d::container::array_remove_swap;
use makepad_box3d::ensure;

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
