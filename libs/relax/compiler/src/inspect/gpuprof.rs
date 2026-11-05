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
    let args = Args::parse(&w);
    let rec = match load(path, &args) {
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
    match query::run(&rec, &args, &load) {
        Ok(a) if w.iter().any(|x| x == "as=md") => {
            for l in query::markdown(&a) {
                println!("{l}");
            }
            0
        }
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

fn load(path: &str, args: &Args) -> Result<Recording, String> {
    validate(args)?;
    std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}")).and_then(|b| Recording::decode(&b))
}

fn validate(args: &Args) -> Result<(), String> {
    let sub = args.pos.first().map(String::as_str).unwrap_or("");
    let schema = makepad_remote::params::gpu_profile(sub).ok_or_else(|| format!("unknown saved GPU-profile query {sub:?}"))?;
    makepad_remote::params::validate(args.kv.iter().map(|(k, _)| k.as_str()), schema)
}

#[cfg(test)]
mod tests {
    #[test]
    fn saved_gpu_profile_rejects_unknown_parameters() {
        // The file does not exist: validation must win over file I/O for every query.
        for sub in ["frames", "frame", "top", "util", "lanes", "runs", "ablate", "diff"] {
            let args = super::Args::parse(&[sub.into(), "typo=1".into()]);
            let error = super::load("missing-profile.mkgp", &args).err().expect("invalid query");
            assert!(error.contains("unknown parameter \"typo\"") && error.contains("known:"), "{sub}: {error}");
        }
        let args = super::Args::parse(&["top".into(), "by=shader".into(), "n=3".into()]);
        assert!(super::validate(&args).is_ok());
        assert!(super::validate(&super::Args::parse(&["top".into(), "sid=1".into()])).is_err());
        assert!(super::validate(&super::Args::parse(&["frame".into(), "n=3".into()])).is_err());
    }
}
