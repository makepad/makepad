// IntoIterator for &Result / &mut Result and Result::iter_mut, as real core (Item = &T / &mut T
// through the Ok value). "| " lines must equal real std's (core_diff.sh).

#[test]
fn a01_ref_result_into_iter() {
    let done: Vec<Option<Result<u32, String>>> = vec![None, Some(Err("x".into())), Some(Ok(2)), Some(Ok(5))];
    let any = done.iter().flatten().flatten().any(|d| *d == 5);
    let sum: u32 = done.iter().flatten().flatten().sum();
    println!("| any {} sum {}", any, sum);
    let r: Result<u32, ()> = Ok(7);
    for v in &r {
        println!("| for ref {}", v);
    }
    let err: Result<u32, &str> = Err("outer");
    println!("| map_or_default {} {}", r.map_or_default(|v| v + 1), err.map_or_default(|v| v + 1));
    let nested: [Result<Result<u32, &str>, &str>; 3] = [Ok(Ok(3)), Ok(Err("inner")), Err("outer")];
    for value in nested {
        println!("| flatten {:?}", value.flatten());
    }
    // SAFETY: err was constructed as Err above.
    println!("| unchecked error {}", unsafe { err.unwrap_err_unchecked() });
}

#[test]
fn a02_mut_result_into_iter() {
    let mut rs: Vec<Result<u32, u8>> = vec![Ok(1), Err(2), Ok(3)];
    for r in rs.iter_mut() {
        for v in r {
            *v *= 10;
        }
    }
    let mut one: Result<String, ()> = Ok("a".into());
    if let Some(s) = one.iter_mut().next() {
        s.push('b');
    }
    println!("| mut {:?} {:?}", rs, one);
}
