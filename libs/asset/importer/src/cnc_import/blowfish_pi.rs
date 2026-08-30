//! Offline generator for the Blowfish initial state.
//!
//! This module is only included by tests.  It computes the hexadecimal
//! fractional digits of pi with Machin's formula, using a fixed-point integer
//! represented by little-endian `u32` limbs.  Guard limbs absorb the rounding
//! from the arctangent series.

const WORDS: usize = 18 + 4 * 256;
const GUARD_WORDS: usize = 12;

pub fn generate() -> ([u32; 18], [[u32; 256]; 4]) {
    let fractional_words = pi_fractional_words(WORDS);
    let mut p = [0; 18];
    p.copy_from_slice(&fractional_words[..18]);
    let mut s = [[0; 256]; 4];
    for (box_index, table) in s.iter_mut().enumerate() {
        let start = 18 + box_index * 256;
        table.copy_from_slice(&fractional_words[start..start + 256]);
    }
    (p, s)
}

fn pi_fractional_words(count: usize) -> Vec<u32> {
    let precision = count + GUARD_WORDS;
    let atan_5 = atan_reciprocal(5, precision);
    let atan_239 = atan_reciprocal(239, precision);
    let mut pi = atan_5;
    mul_small(&mut pi, 16);
    let mut correction = atan_239;
    mul_small(&mut correction, 4);
    sub_assign(&mut pi, &correction);

    (0..count).map(|index| pi[precision - 1 - index]).collect()
}

fn atan_reciprocal(denominator: u32, precision: usize) -> Vec<u32> {
    // One extra limb holds the integer part of the fixed-point value.
    let mut term = vec![0; precision + 1];
    term[precision] = 1;
    let mut high = div_small_active(&mut term, denominator, precision);
    let denominator_squared = denominator * denominator;
    let mut sum = vec![0; precision + 1];
    let mut odd = 1u32;
    let mut add = true;
    while let Some(active_high) = high {
        if add {
            add_assign(&mut sum, &term);
        } else {
            sub_assign(&mut sum, &term);
        }
        let multiplied_high = mul_small_active(&mut term, odd, active_high);
        odd += 2;
        high = div_small_active(&mut term, odd * denominator_squared, multiplied_high);
        add = !add;
    }
    sum
}

fn div_small_active(value: &mut [u32], divisor: u32, high: usize) -> Option<usize> {
    let mut remainder = 0u64;
    for limb in value[..=high].iter_mut().rev() {
        let dividend = (remainder << 32) | *limb as u64;
        *limb = (dividend / divisor as u64) as u32;
        remainder = dividend % divisor as u64;
    }
    (0..=high).rev().find(|&index| value[index] != 0)
}

fn mul_small(value: &mut [u32], multiplier: u32) {
    let mut carry = 0u64;
    for limb in value {
        let product = *limb as u64 * multiplier as u64 + carry;
        *limb = product as u32;
        carry = product >> 32;
    }
}

fn mul_small_active(value: &mut [u32], multiplier: u32, high: usize) -> usize {
    let mut carry = 0u64;
    for limb in &mut value[..=high] {
        let product = *limb as u64 * multiplier as u64 + carry;
        *limb = product as u32;
        carry = product >> 32;
    }
    if carry != 0 && high + 1 < value.len() {
        value[high + 1] = carry as u32;
        high + 1
    } else {
        high
    }
}

fn add_assign(value: &mut [u32], addend: &[u32]) {
    let mut carry = 0u64;
    for (limb, &other) in value.iter_mut().zip(addend) {
        let sum = *limb as u64 + other as u64 + carry;
        *limb = sum as u32;
        carry = sum >> 32;
    }
}

fn sub_assign(value: &mut [u32], subtrahend: &[u32]) {
    let mut borrow = 0u64;
    for (limb, &other) in value.iter_mut().zip(subtrahend) {
        let sub = other as u64 + borrow;
        let current = *limb as u64;
        *limb = current.wrapping_sub(sub) as u32;
        borrow = u64::from(current < sub);
    }
}
