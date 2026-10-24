// LIVEICE: a refused edit (a call to a fn that does not exist yet, E0425) followed by the edit
// that adds the fn above the caller. The caller has closures and runs on another thread; after
// the applied edit its closures must compile from the new version (no "side table: no entry").
//  v1: parse calls `first_end` before it exists: refused, v0 keeps running.
//  v2: `first_end` added above parse (expression ids below it shift): applied.

fn parse(text: &str) -> Result<u32, String> {
    let start = text.find('{').ok_or("no object")?;
    let end = first_end(text, start).ok_or("no object")?;
    let body = &text[start + 1..end];
    let num = |key: &str| body.split(',').find_map(|kv| {
        let (k, v) = kv.split_once(':')?;
        (k.trim() == key).then(|| v.trim().parse::<u32>().map_err(|e| format!("{key}: {e}")))
    });
    let a = num("a").unwrap_or(Ok(0))?;
    let b = num("b").unwrap_or(Ok(0))?;
    Ok(a * 100 + b)
}

#[test]
fn run() {
    let h = std::thread::spawn(|| parse("x {a: 3, b: 4} {a: 5}"));
    println!("out {:?}", h.join().unwrap());
}
