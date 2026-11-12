// `cfg_select!` arms are `pred => { tokens }` or `pred => expr,`. An arm that is not chosen is
// still parsed; an unbraced one is one expression ending at its comma, whatever the position of
// the call (matrix-sdk's send_queue/progress.rs: `feature = ".." => accumulated.len(),` as the
// tail of a block).
fn tail_of_block(n: usize) -> usize {
    let index = {
        cfg_select! {
            feature = "no-such-feature" => n.wrapping_add(40),
            _ => {
                0 // the braced arm is the one chosen
            }
        }
    };
    index + n
}

fn statement(n: &mut usize) {
    cfg_select! {
        feature = "no-such-feature" => *n += 40,
        any() => { *n += 50; *n += 60; }
        _ => *n += 1,
    }
    cfg_select! {
        not(any()) => *n += 2,
        _ => { *n += 70; }
    };
}

fn expression(n: usize) -> usize {
    let a = cfg_select! {
        any() => n + 40,
        _ => n + 3,
    };
    let b = cfg_select! {
        any() => { n + 50 }
        not(any()) => n + 4,
        _ => n + 60,
    };
    a + b
}

cfg_select! {
    any() => { fn item() -> usize { 40 } }
    _ => { fn item() -> usize { 5 } }
}

#[test]
fn unbraced_arms() {
    assert_eq!(tail_of_block(7), 7);
    let mut n = 0;
    statement(&mut n);
    assert_eq!(n, 3);
    assert_eq!(expression(10), 27);
    assert_eq!(item(), 5);
}

// commas inside the expression of an unbraced arm (generic arguments, closure parameters) do not end it
fn commas_inside(n: usize) -> usize {
    let a = cfg_select! {
        any() => std::collections::HashMap::<u8, u16>::new().len() + 40,
        _ => std::collections::HashMap::<u8, u16>::new().len() + 1,
    };
    let f = cfg_select! {
        not(any()) => |x: usize, y: usize| x + y,
        _ => |x: usize, y: usize| x * y,
    };
    let g = {
        cfg_select! {
            any() => (|x: usize, y: usize| x * y)(n, 9),
            _ => (|x: usize, y: usize| x + y)(n, 2),
        }
    };
    a + f(n, 1) + g
}

#[test]
fn commas_in_unbraced_arms() {
    assert_eq!(commas_inside(5), 1 + 6 + 7);
}
