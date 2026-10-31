// async-stream's `stream!` expansion: every `yield` becomes
// `{ if false { break '__check (loop {}); } tx.send(v).await }` inside a labeled block
use std::future::{ready, Future};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

fn run<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut Context::from_waker(Waker::noop())) { return v; }
    }
}

struct Tx<T>(Vec<T>);
impl<T> Tx<T> {
    fn send(&mut self, v: T) -> std::future::Ready<()> { self.0.push(v); ready(()) }
}

#[test]
fn labeled_block_never_break_then_loop_returning() {
    let mut tx = Tx(Vec::new());
    run(async {
        '__check: {
            let mut n = 0;
            loop {
                {
                    #[allow(unreachable_code)]
                    if false { break '__check (loop {}); }
                    tx.send(Ok::<u32, ()>(n)).await
                }
                n += 1;
                if n == 3 { return; }
            }
        }
    });
    assert_eq!(tx.0, vec![Ok(0), Ok(1), Ok(2)]);
}

#[test]
fn labeled_block_never_break_then_loop_breaking() {
    let mut tx = Tx(Vec::new());
    run(async {
        '__check: {
            let mut n = 0;
            loop {
                match n {
                    3 => break,
                    _ => {
                        {
                            #[allow(unreachable_code)]
                            if false { break '__check (loop {}); }
                            tx.send(n).await
                        }
                        n += 1;
                    }
                }
            }
        }
    });
    assert_eq!(tx.0, vec![0, 1, 2]);
}

#[test]
fn labeled_block_never_break_sync() {
    let mut v = Vec::new();
    let r: () = '__check: {
        let mut n = 0;
        loop {
            #[allow(unreachable_code)]
            if false { break '__check (loop {}); }
            v.push(n);
            n += 1;
            if n == 2 { break '__check; }
        }
    };
    assert_eq!(r, ());
    assert_eq!(v, vec![0, 1]);
}

fn early(v: &mut Vec<u32>) -> Result<(), ()> {
    '__check: {
        let mut n = 0;
        loop {
            #[allow(unreachable_code)]
            if false { break '__check (loop {}); }
            v.push(n);
            n += 1;
            if n == 2 { return Err(()); }
        }
    }
}

#[test]
fn labeled_block_never_break_fn_tail() {
    let mut v = Vec::new();
    assert_eq!(early(&mut v), Err(()));
    assert_eq!(v, vec![0, 1]);
}

#[allow(unreachable_code)]
fn loop_never_break(b: bool) -> u32 {
    if b { let x: u32 = loop { break panic!("no") }; x } else { 7 }
}

#[test]
fn loop_break_never_value_takes_the_expected_type() {
    assert_eq!(loop_never_break(false), 7);
    #[allow(unreachable_code)]
    let v = 'a: {
        if false { break 'a (loop {}); }
        break 'a 5u8;
    };
    assert_eq!(v, 5u8);
    #[allow(unreachable_code)]
    let w = loop {
        if true { break 9i16; }
        break (loop {});
    };
    assert_eq!(w, 9i16);
}
