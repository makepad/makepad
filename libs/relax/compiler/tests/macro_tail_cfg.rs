//! A macro call that ends a block expands to an expression whose outer attributes are configured
//! as a statement's (relax issue 23): a false `#[cfg]` removes the expression without resolving
//! the names in it, so the block has no tail; a true one keeps it as the block's value.
macro_rules! trace {
    ($($arg:tt)*) => {
        #[cfg(any())]
        {
            undefined_crate::trace!($($arg)+);
        }
    };
}

macro_rules! doubled {
    ($e:expr) => {
        #[cfg(all())]
        {
            $e * 2
        }
    };
}

fn drain(n: &mut u32) {
    *n += 1;
    trace!("body drained")
}

fn arm(x: u8) -> u32 {
    let mut n = 0;
    match x {
        0 => {
            drain(&mut n);
            trace!("zero", x)
        }
        _ => { trace!(other) }
    }
    n
}

fn twice(x: u32) -> u32 {
    doubled!(x + 1)
}

#[test]
fn cfg_gated_tail_expansions() {
    let mut n = 0;
    drain(&mut n);
    assert_eq!(n, 1, "a disabled tail expansion is removed");
    assert_eq!((arm(0), arm(1)), (1, 0), "in match arm blocks too");
    assert_eq!(twice(4), 10, "an enabled one is the block's value");
    let unit: () = { trace!(in a block) };
    assert_eq!(unit, ());
}
