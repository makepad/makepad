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
