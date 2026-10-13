//! `makepad inspect file=<x.mkgp> gpu prof ...`: queries on a saved GPU recording, no program. (Live
//! `gpu prof ...` is the program's own `gpu` command, registered with the host like the ledger.)

use super::json_str;
use makepad_gpu_trace::query::{self, Args};
use makepad_gpu_trace::Recording;

/// `makepad inspect file=<x.mkgp> gpu prof <query> ...`: answers from a saved recording.
pub fn file_main(path: &str, words: &[String]) -> i32 {
    // accept `gpu prof frames ...` and `prof frames ...` and `frames ...`
    let mut w: Vec<String> = words.to_vec();
    if w.first().map(String::as_str) == Some("gpu") {
        w.remove(0);
    }
    if w.first().map(String::as_str) == Some("prof") {
        w.remove(0);
    }
    let rec = match std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}")).and_then(|b| Recording::decode(&b)) {
        Ok(r) => r,
        Err(e) => {
            println!("{{\"err\":{}}}", json_str(&e));
            return 1;
        }
    };
    let own = rec.clone();
    let load = move |name: &str| -> Result<Recording, String> {
        if name == "file" {
            return Ok(own.clone());
        }
        let bytes = std::fs::read(name).map_err(|e| format!("cannot read {name}: {e}"))?;
        Recording::decode(&bytes)
    };
    match query::run(&rec, &Args::parse(&w), &load) {
        Ok(a) => {
            println!("{{\"measured\":{},\"source\":{}}}", json_str(&a.measured), json_str(&format!("file {path}")));
            for l in a.lines {
                println!("{l}");
            }
            for f in a.findings {
                println!("{{\"finding\":{}}}", json_str(&f));
            }
            0
        }
        Err(e) => {
            println!("{{\"err\":{}}}", json_str(&e));
            1
        }
    }
}
