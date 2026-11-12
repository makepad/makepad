//! `try { }` blocks (nightly feature `try_blocks`): a `?` inside leaves the block, not the fn,
//! with its residual made into the block's type (`residual_into_try_type`: the residual decides
//! the type, not the context), and the block's value is `Try::from_output` of its tail (relax
//! issue 34).
#![feature(try_blocks)]
use std::cell::Cell;
use std::num::ParseIntError;

fn parse_pair(a: &str, b: &str) -> Result<(i32, i32), ParseIntError> {
    try { (a.parse::<i32>()?, b.parse::<i32>()?) }
}

// the fn returns a plain u32: the `?`s only leave the block
fn first_even_times_ten(v: &[u32]) -> u32 {
    let r: Option<u32> = try {
        let x = v.first()?;
        if x % 2 == 1 {
            None::<()>?;
        }
        *x * 10
    };
    r.unwrap_or(7)
}

fn checked(v: &[i64]) -> Result<(), String> {
    for &x in v {
        if x < 0 {
            return Err(format!("negative {x}"));
        }
    }
    Ok(())
}

struct Noisy<'a>(&'a Cell<u32>, u32);
impl Drop for Noisy<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() * 10 + self.1);
    }
}

#[test]
fn values_and_residuals() {
    assert_eq!(parse_pair("4", "-5"), Ok((4, -5)), "the tail goes through from_output");
    assert!(parse_pair("4", "x").is_err(), "a `?` gives the block's residual");
    assert_eq!((first_even_times_ten(&[4]), first_even_times_ten(&[3]), first_even_times_ten(&[])), (40, 7, 7), "a `?` leaves the block, not the fn");

    // no tail: from_output(())
    let ok: Result<(), String> = try { checked(&[1, 2])?; };
    let bad: Result<(), String> = try { checked(&[1, -2])?; };
    assert_eq!((ok, bad), (Ok(()), Err("negative -2".to_string())), "a block without a tail");

    // the residual decides the block's type (homogeneous blocks): no annotation needed
    let r = try { "300".parse::<u8>()? };
    assert_eq!(r.map_err(|e| e.to_string()), Err("number too large to fit in target type".to_string()), "a block typed by its residual");
}

#[test]
fn nesting_closures_and_loops() {
    // an inner `?` leaves the inner block only
    let outer: Option<(Option<u8>, u8)> = try {
        let inner: Option<u8> = try { None::<u8>? + 1 };
        (inner, Some(2u8)?)
    };
    assert_eq!(outer, Some((None, 2)), "nested try blocks");

    // a `?` in a closure returns from the closure
    let r: Option<u32> = try {
        let f = |x: Option<u32>| -> Option<u32> { Some(x? + 1) };
        f(None).unwrap_or(5) + f(Some(1))?
    };
    assert_eq!(r, Some(7), "a closure inside");

    // a `break` passes the block by to the loop
    let mut seen = Vec::new();
    for i in 0..10u32 {
        let r: Option<u32> = try {
            if i == 3 {
                break;
            }
            [10u32, 20][i as usize % 2].checked_sub(i)?
        };
        seen.push(r);
    }
    assert_eq!(seen, [Some(10), Some(19), Some(8)], "a break inside the block");
}

#[test]
fn drops_when_a_residual_leaves() {
    let log = Cell::new(0);
    let r: Option<u32> = try {
        let _a = Noisy(&log, 1);
        let _b = Noisy(&log, 2);
        None::<u32>?
    };
    let _c = Noisy(&log, 3);
    assert_eq!((r, log.get()), (None, 21), "the block's locals drop in reverse order as the `?` leaves it");
}
