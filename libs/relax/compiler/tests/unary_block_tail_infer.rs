//! A unary `!` / `-` passes its expectation to the operand (rustc check_expr_unop), and a block
//! or `if` operand coerces its tail to it: an open tail is unified with the expected type
//! (coerce_from_inference_variable). objc's `msg_send!` is such a block under `if !..`.
fn send<R: Default>() -> R {
    R::default()
}

fn send_result<R: Default>() -> Result<R, String> {
    Ok(R::default())
}

// objc 0.2 `msg_send!`'s expansion shape
macro_rules! msg_send {
    () => {{
        let result;
        match send_result() {
            Err(s) => panic!("{}", s),
            Ok(r) => result = r,
        }
        result
    }};
}

#[test]
fn not_of_a_block_is_the_condition_type() {
    let mut n = 0;
    if !msg_send!() {
        n += 1;
    }
    if !{ send() } {
        n += 1;
    }
    assert_eq!(n, 2);
}

#[test]
fn unary_block_tail_takes_the_let_type() {
    let b: bool = !{ let r = send(); r };
    assert!(b);
    let x: u8 = !{ send() };
    assert_eq!(x, 255);
    let y: i32 = -{ send() };
    assert_eq!(y, 0);
    let c = true;
    let z: bool = !if c { send() } else { send() };
    assert!(z);
}
