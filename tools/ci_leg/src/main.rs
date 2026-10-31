//! `ci-leg smoke <ci.splash> --dir <dir> [--target <package>:<binary>:<cwd>]` | `ci-leg slice <app>
//! <root,...>`: one CI shard, from the root of the tree under test (see the lib).

use makepad_ci_leg::{judge::Judge, slice, smoke};

const USAGE: &str = "usage: ci-leg smoke <ci.splash> --dir <dir> [--target <package>:<binary>:<cwd>] | ci-leg slice <app> <root,...>";

/// "This box cannot run me" (devhub ci: the shard goes to another box).
const UNFIT: i32 = 75;

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
            // the judge's discovery first: the fleet's hubs are heard while the app starts
            let judge = Judge::start();
            // a box off the LAN cannot judge: another box runs this smoke (devhub ci's unfit exit)
            if let Some(why) = judge.unreachable() {
                println!("CI-UNFIT vision judge: {why}");
                std::process::exit(UNFIT);
            }
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
            smoke::run(root, &args[1], &dir, target, judge)
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
