//! Statics of a provided crate reached from JIT-compiled code (provided_static.sh): `memo` runs
//! fns of the fixture lib that are not ops (compiled from source) and use its per-side MEMO;
//! `shared` reaches incompatible SHARED_LOG and `unmarked` reaches unmarked PLAIN_COUNT: both
//! must stop. `tls` uses each side's own per-thread storage. `statics` exercises marked shared
//! public, private, function-local, mutable, Cell-backed and array storage. Each field records
//! op/JIT reads after that mutation; RAW records op/plain/direct reads. The local op calls the
//! same helper as the plain JIT call, so both reach one LOCAL in `local_count_bump`.
//! `static-ref` reaches shared storage through a JIT static reference; `unmarked-ref` must
//! refuse the same initializer for an unmarked atomic before it prints anything.

fn shared_reference() {
    use relax_provided_fixture as fixture;
    use std::sync::atomic::{AtomicU64, Ordering};

    static R: &AtomicU64 = &fixture::SHARED_COUNT;
    fixture::shared_count_add(5);
    let op = fixture::shared_count_snapshot();
    println!("ref op={}/{} addr={}/{}", op[1], R.load(Ordering::Relaxed), R as *const _ as usize, op[0]);
    R.fetch_add(7, Ordering::Relaxed);
    let jit = fixture::shared_count_snapshot();
    println!("ref jit={}/{} addr={}/{}", jit[1], R.load(Ordering::Relaxed), R as *const _ as usize, jit[0]);
}

fn unmarked_reference() {
    use std::sync::atomic::{AtomicU64, Ordering};

    static R: &AtomicU64 = &relax_provided_fixture::PLAIN_COUNT;
    println!("unmarked-ref {}", R.load(Ordering::Relaxed));
}

// The refusing cases live in fns of their own: a refusal is the compile error of the fn that reads
// the static, so main (which every case runs) must not hold one.
fn shared_case() {
    println!("shared {}", relax_provided_fixture::shared_log("from here"));
}

fn unmarked_case() {
    println!("unmarked {}", relax_provided_fixture::PLAIN_COUNT.load(std::sync::atomic::Ordering::Relaxed));
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("shared") => shared_case(),
        Some("unmarked") => unmarked_case(),
        Some("static-ref") => shared_reference(),
        Some("unmarked-ref") => unmarked_reference(),
        Some("statics") => {
            use relax_provided_fixture as fixture;
            use std::sync::atomic::Ordering;

            fixture::shared_count_add(5);
            let op = (fixture::shared_count(), fixture::SHARED_COUNT.load(Ordering::Relaxed));
            fixture::shared_count_bump(7);
            let plain = (fixture::shared_count(), fixture::SHARED_COUNT.load(Ordering::Relaxed));
            fixture::SHARED_COUNT.fetch_add(11, Ordering::Relaxed);
            let direct = (fixture::shared_count(), fixture::SHARED_COUNT.load(Ordering::Relaxed));
            let same_addr = fixture::shared_count_addr() == (&raw const fixture::SHARED_COUNT) as usize;
            println!("count op={}/{} plain={}/{} direct={}/{} same_addr={}", op.0, op.1, plain.0, plain.1, direct.0, direct.1, same_addr as u8);

            fixture::hidden_count_add(5);
            let op = (fixture::hidden_count(), fixture::hidden_count_read());
            fixture::hidden_count_bump(7);
            let plain = (fixture::hidden_count(), fixture::hidden_count_read());
            let same_addr = fixture::hidden_count_addr() == fixture::hidden_count_plain_addr();
            println!("hidden op={}/{} plain={}/{} same_addr={}", op.0, op.1, plain.0, plain.1, same_addr as u8);

            fixture::local_count_add(5);
            let op = (fixture::local_count(), fixture::local_count_bump(0));
            fixture::local_count_bump(7);
            let plain = (fixture::local_count(), fixture::local_count_bump(0));
            println!("local op={}/{} plain={}/{}", op.0, op.1, plain.0, plain.1);

            fixture::raw_set(41);
            let op = (fixture::raw_count(), fixture::raw_read(), unsafe { fixture::RAW });
            unsafe { fixture::RAW = 42 };
            let direct = (fixture::raw_count(), fixture::raw_read(), unsafe { fixture::RAW });
            let same_addr = fixture::raw_addr() == (&raw const fixture::RAW) as usize;
            println!("raw op={}/{}/{} direct={}/{}/{} same_addr={}", op.0, op.1, op.2, direct.0, direct.1, direct.2, same_addr as u8);

            fixture::cells_add(5);
            let op = (fixture::cells_count(), fixture::CELLS.c.get());
            fixture::cells_bump(7);
            let plain = (fixture::cells_count(), fixture::CELLS.c.get());
            fixture::CELLS.c.set(fixture::CELLS.c.get() + 11);
            let direct = (fixture::cells_count(), fixture::CELLS.c.get());
            let same_addr = fixture::cells_addr() == (&raw const fixture::CELLS) as usize;
            println!("cells op={}/{} plain={}/{} direct={}/{} same_addr={}", op.0, op.1, plain.0, plain.1, direct.0, direct.1, same_addr as u8);

            fn array_counts() -> ([u32; 4], [u32; 4]) {
                let mut op = [0; 4];
                let mut direct = [0; 4];
                for i in 0..4 {
                    op[i] = fixture::arr_count(i);
                    direct[i] = fixture::ARR[i].load(Ordering::Relaxed);
                }
                (op, direct)
            }
            for i in 0..4 {
                fixture::arr_add(i, i as u32 + 1);
            }
            let op = array_counts();
            for i in 0..4 {
                fixture::arr_bump(i, (i as u32 + 1) * 2);
            }
            let plain = array_counts();
            for i in 0..4 {
                fixture::ARR[i].fetch_add((i as u32 + 1) * 3, Ordering::Relaxed);
            }
            let direct = array_counts();
            let same_addr = fixture::arr_addr() == (&raw const fixture::ARR) as usize;
            println!("arr op={:?}/{:?} plain={:?}/{:?} direct={:?}/{:?} same_addr={}", op.0, op.1, plain.0, plain.1, direct.0, direct.1, same_addr as u8);
        }
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
