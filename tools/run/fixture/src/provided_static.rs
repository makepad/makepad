//! Statics of a provided crate reached from JIT-compiled code (provided_static.sh): `memo` runs
//! fns of the fixture lib that are not ops (compiled from source) and use its per-side MEMO;
//! `shared` reaches SHARED_LOG, rustc's one copy, which JIT code is refused; `tls` its
//! thread-locals (each side keeps its own per-thread storage).
fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("shared") => println!("shared {}", relax_provided_fixture::shared_log("from here")),
        Some("tls") => {
            let a = relax_provided_fixture::tls_bump(1);
            let b = relax_provided_fixture::tls_bump(2);
            let other = std::thread::spawn(|| relax_provided_fixture::tls_bump(3)).join().unwrap();
            println!("tls {:?} {:?} other thread {:?}", a, b, other);
        }
        _ => {
            let a = relax_provided_fixture::memo_square(7);
            let b = relax_provided_fixture::memo_square(7);
            let c = relax_provided_fixture::memo_square(9);
            println!("memo {} {} {} len {}", a, b, c, relax_provided_fixture::memo_len());
        }
    }
}
