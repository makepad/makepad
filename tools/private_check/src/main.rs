//! Fails when a tracked file names a private address, a personal path or a private host.
//!
//! Live addresses belong in local config (`~/.makepad`) or discovery. Tests and docs use the
//! documentation range 192.0.2.x. Code that needs a real private address, like a test of a LAN
//! rule or the hotspot's own subnet, uses the `.77.` ones: 10.77.0.x, 192.168.77.x, 172.16.77.x.
use std::process::{Command, ExitCode};

/// Third-party text whose dotted numbers aren't addresses.
const SKIP_PREFIXES: &[&str] = &["libs/rapier/vendor/", "tools/private_check/"];
const SKIP_SUFFIXES: &[&str] = &[".wast"];
/// Where `/home/arch` is what the image provisions, not somebody's machine.
const ARCH_HOME_OK: &str = "tools/arch_usb/";
const WORDS: &[&str] = &[
    "/Users/admin",
    "C:\\Users\\playe",
    "C:\\\\Users\\\\playe",
    "/home/arch",
    "mini230",
    "mbpro",
    "10.0.0.{",
];

fn is_private(o: [u32; 4]) -> bool {
    o[0] == 10 || (o[0] == 192 && o[1] == 168) || (o[0] == 172 && (16..32).contains(&o[1]))
}

fn is_allowed(o: [u32; 4]) -> bool {
    matches!(o, [10, 77, 0, _] | [192, 168, 77, _] | [172, 16, 77, _])
}

/// Every dotted quad in `line` that is private and isn't one of the allowed ones.
fn private_addresses(line: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        if !line[i].is_ascii_digit() || (i > 0 && (line[i - 1].is_ascii_digit() || line[i - 1] == b'.')) {
            i += 1;
            continue;
        }
        let mut o = [0u32; 4];
        let mut at = i;
        let mut parts = 0;
        while parts < 4 {
            let start = at;
            let mut value = 0u32;
            while at < line.len() && line[at].is_ascii_digit() && at - start < 4 {
                value = value * 10 + (line[at] - b'0') as u32;
                at += 1;
            }
            if at == start || value > 255 {
                break;
            }
            o[parts] = value;
            parts += 1;
            if parts < 4 {
                if at < line.len() && line[at] == b'.' {
                    at += 1;
                } else {
                    break;
                }
            }
        }
        let ended = at >= line.len() || !line[at].is_ascii_digit();
        if parts == 4 && ended && is_private(o) && !is_allowed(o) {
            out.push(format!("{}.{}.{}.{}", o[0], o[1], o[2], o[3]));
        }
        i += 1;
    }
    out
}

fn contains(line: &[u8], word: &[u8]) -> bool {
    line.windows(word.len()).any(|w| w == word)
}

fn main() -> ExitCode {
    let listed = match Command::new("git").args(["ls-files", "-z"]).output() {
        Ok(output) if output.status.success() => output.stdout,
        _ => {
            eprintln!("private check: run it inside the repository");
            return ExitCode::from(2);
        }
    };
    let mut hits = 0;
    for path in listed.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let path = String::from_utf8_lossy(path);
        if SKIP_PREFIXES.iter().any(|p| path.starts_with(p)) || SKIP_SUFFIXES.iter().any(|s| path.ends_with(s)) {
            continue;
        }
        let Ok(bytes) = std::fs::read(path.as_ref()) else { continue };
        // A NUL early on means a binary file, as git decides it.
        if bytes.iter().take(8000).any(|b| *b == 0) {
            continue;
        }
        for (n, line) in bytes.split(|b| *b == b'\n').enumerate() {
            let mut found = private_addresses(line);
            for word in WORDS {
                if contains(line, word.as_bytes()) && !(*word == "/home/arch" && path.starts_with(ARCH_HOME_OK)) {
                    found.push((*word).to_string());
                }
            }
            for what in found {
                println!("{path}:{}: {what}", n + 1);
                hits += 1;
            }
        }
    }
    if hits > 0 {
        println!("private check: {hits} hits. Use local config or discovery; tests and docs use 192.0.2.x.");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
