//! `cfg_select!` arms (relax issue 121): an arm's body is a braced block or an expression, and a
//! comma may follow either, the last arm's included; the arms not chosen are parsed too. A comma
//! after an arm without braces that was not chosen stopped the crate's load in statement position
//! ("expected `;` or `}` after expression"), where a `cfg_select!` ending in `_ => (),` stops. The
//! predicates hold on the hosts agree runs on (unix).

fn tail_unit_arm() -> u32 {
    cfg_select! {
        unix => { 1 }
        windows => { 2 }
        _ => (),
    }
}

fn tail_bare_arms() -> u32 {
    cfg_select! {
        unix => 1,
        windows => 2,
        _ => 3,
    }
}

fn tail_last_arm_chosen() -> u32 {
    cfg_select! {
        windows => { 2 }
        _ => 1
    }
}

cfg_select! {
    unix => { fn item() -> u32 { 1 } }
    _ => { fn item() -> u32 { 2 } },
}

#[test]
fn arms_with_and_without_braces_take_a_comma() {
    assert_eq!(tail_unit_arm(), 1, "a block's tail, `_ => (),` last");
    assert_eq!(tail_bare_arms(), 1, "a block's tail, every arm an expression with its comma");
    assert_eq!(tail_last_arm_chosen(), 1, "the last arm chosen, no comma");
    assert_eq!(item(), 1, "items, a comma after a braced arm");
    let x = cfg_select! { windows => 5, _ => 6, };
    assert_eq!(x, 6, "an expression");
    let v = [cfg_select! { unix => 7, _ => 8, }];
    assert_eq!(v, [7], "an array element");
    let mut seen = Vec::new();
    cfg_select! {
        unix => { seen.push("unix"); }
        windows => seen.push("windows"),
        _ => (),
    }
    seen.push("after");
    assert_eq!(seen, ["unix", "after"], "a statement");
}
