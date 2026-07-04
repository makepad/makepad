// Port of box3d/test/test_bitset.c

use makepad_box3d::bitset::*;
use makepad_box3d::ctz::{clz32, lower_power_of_2_exponent};
use makepad_box3d::ensure;

const COUNT: usize = 169;

#[test]
fn test_bit_math() {
    let r1 = clz32(9);
    ensure!(r1 == 31 - 3);

    for i in 1..1000i32 {
        let e1 = lower_power_of_2_exponent(i);
        let e2 = (i as f32).log2().floor() as i32;
        ensure!(e1 == e2);
    }
}

#[test]
fn test_bit_set() {
    let mut bit_set = create_bit_set(COUNT as u32);

    set_bit_count_and_clear(&mut bit_set, COUNT as u32);
    let mut values = [false; COUNT];

    let mut i1: i32 = 0;
    let mut i2: i32 = 1;
    set_bit(&mut bit_set, i1 as u32);
    values[i1 as usize] = true;

    while (i2 as usize) < COUNT {
        set_bit(&mut bit_set, i2 as u32);
        values[i2 as usize] = true;
        let next = i1 + i2;
        i1 = i2;
        i2 = next;
    }

    for i in 0..COUNT {
        let value = get_bit(&bit_set, i as u32);
        ensure!(value == values[i]);
    }

    destroy_bit_set(&mut bit_set);
}
