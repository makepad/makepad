//! A macro call that ends a block expands to an expression whose outer attributes are configured
//! as a statement's (relax issue 23): a false `#[cfg]` removes the expression without resolving
//! the names in it, so the block has no tail; a true one keeps it as the block's value. So does
//! one whose expansion is another macro's call, whose expansion has the `#[cfg]` (relax issue 27).
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

macro_rules! warned {
    ($($arg:tt)*) => { trace!(warned: $($arg)*) };
}

macro_rules! warned_twice {
    ($($arg:tt)*) => { warned!($($arg)*) };
}

macro_rules! quadrupled {
    ($e:expr) => { doubled!($e * 2) };
}

fn quad(x: u32) -> u32 {
    quadrupled!(x)
}

fn in_range(x: u32) -> u32 {
    let mut n = 0;
    if x > 9 {
        n += 1;
        warned!("not in range")
    }
    if x > 99 {
        n += 1;
        warned_twice!("far out of range")
    }
    n
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
    assert_eq!((in_range(5), in_range(50), in_range(500)), (0, 1, 2), "an expansion that is another call: its disabled expansion is removed, two levels down too");
    assert_eq!(quad(3), 12, "an enabled one through a nested call is the block's value");
}
