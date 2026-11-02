//! `ci-leg smoke <ci.splash> --dir <dir> [--target <package>:<binary>:<cwd>]` | `ci-leg slice <app>
//! <root,...>`: one CI shard, from the root of the tree under test (see the lib).

use makepad_ci_leg::{slice, smoke};

const USAGE: &str = "usage: ci-leg smoke <ci.splash> --dir <dir> [--target <package>:<binary>:<cwd>] | ci-leg slice <app> <root,...>";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            println!("CI-FAIL ci-leg: no working directory: {e}");
            std::process::exit(1);
        }
    };
    let code = match args.first().map(String::as_str) {
        Some("smoke") if args.len() >= 2 => {
            let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
            let dir = flag("--dir").unwrap_or_else(|| args[1].rsplit_once('/').map_or(".", |(d, _)| d).to_string());
            let target = match flag("--target") {
                Some(t) => match smoke::Target::parse(&t) {
                    Some(t) => Some(t),
                    None => {
                        println!("CI-FAIL ci-leg: bad --target {t} ({USAGE})");
                        std::process::exit(2);
                    }
                },
                None => None,
            };
            smoke::run(root, &args[1], &dir, target)
        }
        Some("slice") if args.len() == 3 => {
            let roots: Vec<String> = args[2].split(',').filter(|s| !s.is_empty()).map(str::to_string).collect();
            slice::run(root, &args[1], &roots)
        }
        _ => {
            println!("CI-FAIL ci-leg: {USAGE}");
            2
        }
    };
    std::process::exit(code);
}
