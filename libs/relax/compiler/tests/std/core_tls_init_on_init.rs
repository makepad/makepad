// std-core differential test (core_diff.sh): thread_local init that re-enters during
// initialisation (rustc tests/ui/threads-sendsync/tls-init-on-init.rs): the outer value replaces
// the inner one, which drops while the key is alive; the slot reads destroyed while the last
// value drops.
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

struct Foo {
    cnt: usize,
}

thread_local!(static FOO: Foo = Foo::init());

static CNT: AtomicUsize = AtomicUsize::new(0);
static DROPS: AtomicUsize = AtomicUsize::new(0);

impl Foo {
    fn init() -> Foo {
        let cnt = CNT.fetch_add(1, Ordering::SeqCst);
        if cnt == 0 {
            FOO.with(|_| {});
        }
        Foo { cnt: cnt }
    }
}

impl Drop for Foo {
    fn drop(&mut self) {
        if self.cnt == 1 {
            FOO.with(|foo| assert_eq!(foo.cnt, 0));
            DROPS.fetch_add(1, Ordering::SeqCst);
        } else {
            assert_eq!(self.cnt, 0);
            if FOO.try_with(|_| ()).is_ok() {
                panic!("should not be in valid state");
            }
            DROPS.fetch_add(10, Ordering::SeqCst);
        }
    }
}

#[test]
fn a01() {
    let r = thread::spawn(|| {
        FOO.with(|f| f.cnt)
    })
    .join();
    println!("| joined ok {} value {:?} drops {} inits {}", r.is_ok(), r.ok(), DROPS.load(Ordering::SeqCst), CNT.load(Ordering::SeqCst));
}
