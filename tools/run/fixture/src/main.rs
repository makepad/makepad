//! The fixture's program: `cargo run -p relax-provided-fixture` (all rustc) and
//! `makepad run relax-provided-fixture` (this file by Relax, the lib bound to the loader) print
//! the same lines except `bound`.
use relax_provided_fixture::*;

fn main() {
    println!("bound {}", bound());
    println!("add {}", add(-3, 1 << 40));
    println!("mix {}", mix(1.5, 2.25, 3, true, 'a'));
    println!("scale {:?}", scale(P { x: 1.0, y: -2.0 }, 3.0));
    let b = big_make(10);
    println!("big {:?} {}", b.a, big_sum(b));
    println!("text_len {}", text_len("héllo"));
    println!("sum {}", sum(&[1, 2, 3, 4_000_000_000]));
    println!("flip {:?} {:?}", flip(Kind::A), flip(Kind::B));
    {
        let mut r = Registry::new();
        println!("empty {}", r.is_empty());
        println!("ids {} {} {}", r.add("one"), r.add("two"), r.add("one"));
        println!("add_all {}", r.add_all(["three", "four", "two"]));
        println!("len {} name_len {}", r.len(), r.name_len(2));
        let moved = r;
        let boxed = Box::new(moved);
        println!("boxed len {}", boxed.len());
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| boxed.boom(7)));
        println!("caught {}", caught.is_err());
        println!("still {}", boxed.boom(0));
    }
    let v = vec![Registry::default(), Registry::new()];
    drop(v);
    println!("drops {}", drops());
    println!("maybe {:?} {:?}", maybe(0), maybe(5));
    for io in ios() { println!("io {:?} {:?}", io.buffer_index(), io); }
    {
        use std::hash::{Hash, Hasher};
        let (a, b) = (site("a.rs", 3), site("b.rs", 4));
        let mut set = std::collections::HashSet::new();
        set.insert(a.clone());
        set.insert(b.clone());
        set.insert(a.clone());
        println!("sites {} {} {}", set.len(), set.contains(&b), set.contains(&site("c.rs", 1)));
        // the value is std's hasher's (Relax's std hashes differently from rustc's): equal
        // values hash alike, unequal ones differ
        let hash = |x: &dyn Fn(&mut std::collections::hash_map::DefaultHasher)| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            x(&mut h);
            h.finish()
        };
        let ha = hash(&|h| a.hash(h));
        println!("site hash {} {}", ha == hash(&|h| a.clone().hash(h)), ha == hash(&|h| b.hash(h)));

        println!("site {:?} {} {:#?}", a, b, b);
    }
    {
        // a closure of this side runs on a native thread of the provided crate, owning a String
        // and a handle it drops there; a refused one is dropped unrun
        use std::sync::{Arc, Mutex};
        let here = std::thread::current().id();
        let seen = Arc::new(Mutex::new(None));
        let (out, text) = (seen.clone(), String::from("moved across"));
        let guard = token(true).expect("granted");
        let ran = spawn(move || {
            let _guard = guard;
            *out.lock().unwrap() = Some((std::thread::current().id(), text));
        }, false);
        let got = seen.lock().unwrap().take();
        println!("spawn ran {} elsewhere {} text {:?} live {}", ran, got.as_ref().is_some_and(|g| g.0 != here), got.map(|g| g.1), live_tokens());
        let flag = Arc::new(Mutex::new(false));
        let (f2, guard) = (flag.clone(), token(true).expect("granted"));
        let refused = spawn(move || { let _guard = guard; *f2.lock().unwrap() = true; }, true);
        println!("refused {} ran {} live {} none {}", refused, *flag.lock().unwrap(), live_tokens(), token(false).is_none());
    }
    {
        // owned heap values cross as words: built on one side, used, grown and dropped on the other
        let mut t = owned_text(30);
        t.push_str(" and grown past its capacity on the caller's side");
        println!("owned text {:?} {}", t, t.len());
        let mut l = owned_list(5);
        l.push(Item { a: 99, b: 1, c: true });
        println!("owned list {} {:?} {:?}", l.len(), l[2], l.last());
        println!("owned empty {:?} {:?}", owned_text(0).len(), owned_list(0));
        println!("consume {}", owned_consume(String::from("built here, freed there")));
        let d = owned_doc(3, true).unwrap();
        println!("doc {} {} {} {:?}", d.name, d.items.len(), d.closure, d.items[1]);
        println!("doc none {}", owned_doc(3, false).is_none());
        // many round trips of big buffers both ways: a leak or double free shows (growth, abort)
        let before = peak_rss();
        let mut total = 0usize;
        for i in 0..20_000usize {
            let s = owned_text(100_000 + i % 7);
            total += s.len();
            total += owned_consume("x".repeat(100_000 + i % 5));
            total += owned_list(2_000).len();
        }
        println!("round trips {} growth under 96 MiB: {}", total, peak_rss() - before < 96 * 1024 * 1024);
    }
    println!("asm {}", asm_twice(40));
}
