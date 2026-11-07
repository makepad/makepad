//! What a test run's output says it ran: libtest's `test result:` lines (cargo's and the JIT
//! harness's) and the `harness = false` targets (the JIT names them; under cargo, a target that
//! ran and said nothing of libtest's is one). A run that executed no test is no pass (a filter
//! that matched nothing passed silently, 11-03).

/// The tally of one run's output, fed line by line (a stream) or whole.
#[derive(Default, Debug, PartialEq)]
pub struct Tally {
    /// `test result:` lines seen
    pub results: u64,
    /// tests that ran: passed + failed over those lines
    pub executed: u64,
    /// `harness = false` targets that ran (programs of their own: what they test is unknown)
    pub plain: u64,
    /// a target cargo said it runs (`Running <target> (<binary>)`) that said nothing of libtest's
    /// yet: a `harness = false` program, once the next target starts or the output ends (cargo
    /// does not name them: a scope test that printed no `test result:` was "no test ran", 11-07)
    quiet_target: bool,
    partial: Vec<u8>,
}

impl Tally {
    pub fn of(log: &str) -> Tally {
        let mut t = Tally::default();
        t.feed(log.as_bytes());
        t.feed(b"\n");
        t
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &b in bytes {
            if b != b'\n' {
                // a summary line is short; a long line is no summary
                if self.partial.len() < 512 { self.partial.push(b); }
                continue;
            }
            let line = std::mem::take(&mut self.partial);
            self.line(String::from_utf8_lossy(&line).trim());
        }
    }

    fn line(&mut self, line: &str) {
        if line.starts_with("Running ") || line.starts_with("Doc-tests ") {
            self.plain += u64::from(std::mem::take(&mut self.quiet_target));
            if line.ends_with("(harness = false)") {
                self.plain += 1;
            } else {
                self.quiet_target = line.starts_with("Running ");
            }
            return;
        }
        // libtest's first line of a target (`running 3 tests`, `running 1 test`)
        if line.strip_prefix("running ").and_then(|r| r.strip_suffix(" tests").or_else(|| r.strip_suffix(" test"))).is_some_and(|n| n.parse::<u64>().is_ok()) {
            self.quiet_target = false;
            return;
        }
        let Some(rest) = line.strip_prefix("test result: ") else { return };
        self.quiet_target = false;
        let count = |word: &str| rest.split([';', '.']).filter_map(|p| p.trim().strip_suffix(word)?.trim().parse::<u64>().ok()).next();
        if let (Some(passed), Some(failed)) = (count(" passed"), count(" failed")) {
            self.results += 1;
            self.executed += passed + failed;
        }
    }

    /// Why a test command's run is no pass although it exited 0: it executed no test. None when
    /// it ran one or a `harness = false` target, when it only lists them, or for any other command.
    pub fn no_pass(&self, cmd: &[String]) -> Option<String> {
        let at = test_word(cmd)?;
        let args = cmd[at..].iter().position(|w| w == "--").map_or(&[][..], |p| &cmd[at + p + 1..]);
        let plain = self.plain + u64::from(self.quiet_target);
        if self.executed > 0 || plain > 0 || args.iter().any(|a| a == "--list") { return None; }
        // libtest's options that take a value: the value is no filter
        let valued = ["--skip", "--test-threads", "--format", "--color", "--logfile", "-Z", "--shuffle-seed"];
        let filter = args.iter().enumerate().any(|(i, a)| !a.starts_with('-') && (i == 0 || !valued.contains(&args[i - 1].as_str())));
        let why = if filter {
            format!("no test matched {}", args.join(" "))
        } else if self.results == 0 {
            "no test ran: the output has no test result".to_string()
        } else {
            "no test ran".to_string()
        };
        let words = &cmd[at + 1..];
        let pkg = words.iter().position(|w| w == "-p").and_then(|p| words.get(p + 1)).or_else(|| words.first().filter(|w| !w.starts_with('-')));
        Some(match pkg.and_then(|pkg| bin_hint(pkg, words)) {
            Some(hint) => format!("{why}: {hint}"),
            None => why,
        })
    }
}

/// Where a command's `test` subcommand is: `cargo test ..`, `makepad test ..`, or the makepad
/// binary through cargo (`cargo run .. --bin makepad -- test ..`). None: not a test command.
/// What a run of `--bin B`'s no tests was probably after, for a command's words after its `test`.
pub fn bin_hint(pkg: &str, words: &[String]) -> Option<String> {
    let bin = words.iter().take_while(|w| *w != "--").position(|w| w == "--bin").and_then(|at| words.get(at + 1))?;
    Some(format!("--bin selects that bin's unit tests; `makepad run {pkg} --bin {bin} -- args` or `devhub live start {pkg} --bin {bin} -- args` runs the program"))
}

pub fn test_word(cmd: &[String]) -> Option<usize> {
    let w: Vec<&str> = cmd.iter().map(String::as_str).collect();
    (1..w.len()).find(|&i| w[i] == "test" && ((i == 1 && w[0] == "cargo") || w[i - 1].rsplit(['/', '\\']).next() == Some("makepad") || (w[i - 1] == "--" && w[..i].contains(&"makepad"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn a_run_that_executed_no_test_is_no_pass() {
        let ok = "running 3 tests\n...\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.00s\n";
        assert_eq!((Tally::of(ok).results, Tally::of(ok).executed), (2, 3));
        assert_eq!(Tally::of(ok).no_pass(&args("cargo test -p x -- foo")), None);
        let none = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.00s\n";
        assert_eq!(Tally::of(none).no_pass(&args("cargo test -p x -- foo --exact")).as_deref(), Some("no test matched foo --exact"));
        let jit = "cargo run --profile host -q -p makepad-run --bin makepad -- test x --lib -- --skip slow foo";
        assert_eq!(Tally::of(none).no_pass(&args(jit)).as_deref(), Some("no test matched --skip slow foo"));
        assert_eq!(Tally::of(none).no_pass(&args("cargo test -p x --lib -- --skip slow")).as_deref(), Some("no test ran"));
        assert_eq!(Tally::of("").no_pass(&args("cargo test -p x")).as_deref(), Some("no test ran: the output has no test result"));
        assert_eq!(Tally::of("").no_pass(&args("cargo test -p x -- --list")), None);
        assert_eq!(Tally::of("").no_pass(&args("bash bench.sh")), None, "not a test command");
        // the bin's own unit tests, none: it names what runs the program
        let hint = "--bin selects that bin's unit tests; `makepad run x --bin b -- args` or `devhub live start x --bin b -- args` runs the program";
        assert_eq!(Tally::of(none).no_pass(&args("cargo test --release -p x --bin b")), Some(format!("no test ran: {hint}")));
        assert_eq!(Tally::of(none).no_pass(&args("cargo run --profile host -p makepad-run --bin makepad -- test x --bin b -- foo")), Some(format!("no test matched foo: {hint}")));
        // the JIT's own single-file summary, and a harness = false target
        assert_eq!(Tally::of("test result: 10 passed; 0 failed. compiled 185 fns (35708 bytes)\n").executed, 10);
        assert_eq!(Tally::of("     Running tests/bench.rs (harness = false)\n").no_pass(&args("makepad test x")), None);
        // cargo's harness = false target: a target that ran and said nothing of libtest's, before
        // the next target or at the end; a libtest target that ran no test is none
        let cargo = |tail: &str| format!("     Running unittests src/lib.rs (target/release/deps/x-0123)\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n\n{tail}");
        let close = cargo("     Running tests/scope_close.rs (target/release/deps/scope_close-4567)\nscope close: 12 checks ok\n");
        assert_eq!(Tally::of(&close).no_pass(&args("cargo test -p x")), None);
        let between = format!("{close}   Doc-tests x\n\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n");
        assert_eq!((Tally::of(&between).plain, Tally::of(&between).no_pass(&args("cargo test -p x"))), (1, None));
        assert_eq!(Tally::of(&cargo("")).no_pass(&args("cargo test -p x")).as_deref(), Some("no test ran"));
        let one = "     Running tests/a.rs (target/release/deps/a-1)\n\nrunning 1 test\ntest t ... ok\n\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n";
        assert_eq!((Tally::of(one).plain, Tally::of(one).executed), (0, 1));
        // fed in pieces, as a stream
        let mut t = Tally::default();
        for piece in ["test res", "ult: FAILED. 0 passed; 2 fa", "iled; 0 ignored\n"] { t.feed(piece.as_bytes()); }
        assert_eq!((t.results, t.executed), (1, 2));
    }

    #[test]
    fn test_commands_are_told_apart() {
        assert_eq!(test_word(&args("cargo test --release -p makepad-devhub")), Some(1));
        assert_eq!(test_word(&args("cargo run --profile host -q -p makepad-run --bin makepad -- test makepad-devhub --lib")), Some(10));
        assert_eq!(test_word(&args("/x/target/host/makepad test relax-base")), Some(1));
        assert_eq!(test_word(&args("cargo run -p tool -- test")), None);
        assert_eq!(test_word(&args("bash run.sh test")), None);
        assert_eq!(test_word(&args("cargo build -p test")), None);
    }
}
