// A fn item called behind references (map_build's par_map_reduce: `identity: &ID` with
// ID = `Vec::new` / `NodePass::default`, called as `identity()` in a scoped thread): rustc's
// `<&F as Fn>::call` forwards to the item. Relax refused it ("unsupported call through a pointer").
// rustc prints the same lines.
#[derive(Default, Debug)]
struct Pass {
    n: u32,
}

fn fold<ID, T>(identity: ID, k: u32) -> Vec<T>
where
    ID: Fn() -> T + Sync,
    T: Send,
{
    std::thread::scope(|scope| {
        let identity = &identity;
        let hs: Vec<_> = (0..k).map(|_| scope.spawn(move || identity())).collect();
        hs.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

fn call_twice<F: Fn() -> u32>(f: &&F) -> u32 {
    f() + (**f)()
}

fn seven() -> u32 {
    7
}

#[test]
fn fn_item_by_ref() {
    let v: Vec<Vec<u8>> = fold(Vec::new, 3);
    println!("| vec_new {:?}", v);
    let p: Vec<Pass> = fold(Pass::default, 2);
    println!("| default {:?}", p.iter().map(|x| x.n).collect::<Vec<_>>());
    println!("| twice {}", call_twice(&&seven));
}
