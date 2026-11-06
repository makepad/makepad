// Branches meet in CoerceMany (rustc coercion.rs try_find_coercion_lub): a later branch coerces to
// the earlier branches' type, here `&String` to `&str` by deref. apps/mail's argument loop:
// `match arg.strip_prefix(..) { Some(v) => v, None if .. => args.next()?, None => continue }`. A
// `?` branch takes an earlier arm's type as its hint, and a hint coercion that holds makes that
// the `?`'s type. The same through if/else, with and without an expected type, a `?` in a block
// arm, and the `?` arm first (the earlier arm is coerced then).
// Run with `makepad test` and rustc --test; lines "| " are compared.

fn sizes(args: &[String]) -> Option<Vec<usize>> {
    let mut args = args.iter();
    let mut out = Vec::new();
    while let Some(arg) = args.next() {
        let value = match arg.strip_prefix("--size=") {
            Some(value) => value,
            None if arg == "--size" => args.next()?,
            None => continue,
        };
        out.push(value.parse().ok()?);
    }
    Some(out)
}

fn expected_match(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    let first = it.next()?;
    let v: &str = match first.strip_prefix("=") {
        Some(v) => v,
        None => it.next()?,
    };
    Some(v.to_uppercase())
}

fn block_arm(args: &[String]) -> Option<usize> {
    let mut it = args.iter();
    let first = it.next()?;
    let v = match first.strip_prefix("-") {
        Some(v) => v,
        None => {
            let n = it.next()?;
            n
        }
    };
    Some(v.len())
}

fn try_arm_first(args: &[String], take: bool) -> Option<String> {
    let mut it = args.iter();
    let v = match take {
        true => it.next()?,
        false => "none",
    };
    Some(format!("{v}!"))
}

fn if_else(args: &[String], c: bool) -> Option<String> {
    let mut it = args.iter();
    let v = if c { "then" } else { it.next()? };
    Some(v.to_string())
}

fn if_else_expected(args: &[String], c: bool) -> Option<String> {
    let mut it = args.iter();
    let v: &str = if c { "then" } else { it.next()? };
    Some(v.to_string())
}

fn if_else_owned(s: &String, c: bool) -> usize {
    let v = if c { "lit" } else { s };
    v.len()
}

#[test]
fn branches_meet_by_deref() {
    let a = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    println!("| sizes {:?} {:?} {:?}", sizes(&a(&["--size=3", "x", "--size", "12"])), sizes(&a(&["--size"])), sizes(&a(&["--size", "q"])));
    println!("| expected {:?} {:?} {:?}", expected_match(&a(&["=ab"])), expected_match(&a(&["x", "cd"])), expected_match(&a(&["x"])));
    println!("| block arm {:?} {:?}", block_arm(&a(&["-abc"])), block_arm(&a(&["x", "hello"])));
    println!("| try arm first {:?} {:?} {:?}", try_arm_first(&a(&["one"]), true), try_arm_first(&a(&[]), true), try_arm_first(&a(&[]), false));
    println!("| if/else {:?} {:?} {:?}", if_else(&a(&["e"]), true), if_else(&a(&["else"]), false), if_else(&a(&[]), false));
    println!("| if/else expected {:?} {:?} {:?}", if_else_expected(&a(&["e"]), true), if_else_expected(&a(&["else"]), false), if_else_expected(&a(&[]), false));
    println!("| if/else owned {} {}", if_else_owned(&"four".to_string(), true), if_else_owned(&"four".to_string(), false));
}
