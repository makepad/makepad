// An edition-2015 decoder's `try!`-style macro used as a statement on a bare `Err(e)`:
// `match Err(E) { Ok(val) => val, Err(err) => return Err(err) }`. Nothing but the `!` arm reaches
// `Ok`'s type, so it is diverging and falls back (rustc fallback.rs) rather than failing
// `_: Sized`. The same holds for an `if` whose other branch is `!`.
use std::io::Read;

#[derive(Debug, PartialEq)]
struct E;

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(val) => val,
            Err(err) => return Err(err),
        }
    };
}

fn read_some<R: Read>(mut rdr: R) -> Result<usize, E> {
    let mut buf = [0u8; 8];
    let n = tri!(rdr.read(&mut buf).map_err(|_| E));
    if n == 0 {
        tri!(Err(E));
    }
    Ok(n)
}

fn never_arm_first(fail: bool) -> Result<u32, E> {
    if fail {
        match Err(E) {
            Err(err) => return Err(err),
            Ok(val) => val,
        };
    }
    Ok(1)
}

fn never_then_branch(fail: bool) -> Result<u32, E> {
    if fail { return Err(E); } else { None.unwrap() };
    Ok(2)
}

fn never_else_branch(fail: bool) -> Result<u32, E> {
    if !fail { None.unwrap() } else { return Err(E) };
    Ok(3)
}

#[test]
fn never_arm_reaches_only_variable() {
    assert_eq!(read_some(&b"abcd"[..]), Ok(4), "a read value passes the macro");
    assert_eq!(read_some(&b""[..]), Err(E), "the bare Err(e) statement returns it");
    assert_eq!(never_arm_first(true), Err(E), "a `!` arm before the value arm");
    assert_eq!(never_arm_first(false), Ok(1), "the guarded match is skipped");
    assert_eq!(never_then_branch(true), Err(E), "a `!` then-branch without a tail");
    assert_eq!(never_else_branch(true), Err(E), "a `!` else-branch");
}
