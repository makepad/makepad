// `Result::flatten` (std since 1.89), `Result::unwrap_err_unchecked` (1.58) and `map_or_default`
// on `Result` and `Option` (1.98). matrix-sdk-sqlite's crypto store flattens the
// `Result<Result<usize, Error>, Error>` of a transaction.
#[derive(Debug, PartialEq)]
struct E(u8);

fn nested(n: u8) -> Result<Result<usize, E>, E> {
    match n {
        0 => Ok(Ok(7)),
        1 => Ok(Err(E(1))),
        _ => Err(E(2)),
    }
}

#[test]
fn flatten() {
    assert_eq!(nested(0).flatten(), Ok(7));
    assert_eq!(nested(1).flatten(), Err(E(1)));
    assert_eq!(nested(2).flatten(), Err(E(2)));
    const C: Result<u8, ()> = Ok::<Result<u8, ()>, ()>(Ok(3)).flatten();
    assert_eq!(C, Ok(3));
}

#[test]
fn unwrap_err_unchecked() {
    let r: Result<u8, String> = Err("no".to_string());
    assert_eq!(unsafe { r.unwrap_err_unchecked() }, "no");
}

#[test]
fn map_or_default() {
    let ok: Result<&str, E> = Ok("four");
    assert_eq!(ok.map_or_default(|s| s.len()), 4);
    let err: Result<&str, E> = Err(E(1));
    assert_eq!(err.map_or_default(|s| s.len()), 0);
    assert_eq!(Some("ab").map_or_default(|s| s.to_uppercase()), "AB");
    assert_eq!(None::<&str>.map_or_default(|s| s.to_uppercase()), "");
}
