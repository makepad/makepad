//! A run's argv: its irreducible exec size, before a node adds its environment and wrappers, and
//! the plain form a Windows node runs it in.

use std::path::{Path, PathBuf};
use crate::os::ExecLimits;

/// The same interpreter word the node can replace with a file-reading command.
pub(super) fn script_word(argv: &[String]) -> Option<(usize, bool)> {
    let name = Path::new(argv.first()?).file_name()?.to_str()?;
    let shell = matches!(name, "sh" | "bash" | "dash" | "zsh" | "ksh" | "ash");
    let python = name.strip_prefix("python").is_some_and(|s| s.bytes().all(|b| b.is_ascii_digit() || b == b'.'));
    if !shell && !python { return None; }
    let i = argv.windows(2).enumerate().skip(1).filter_map(|(i, pair)| {
        let options = pair[0].strip_prefix('-')?;
        (options.bytes().all(|b| b.is_ascii_alphabetic()) && if shell { options.contains('c') } else { options.ends_with('c') }).then_some(i + 1)
    }).max_by_key(|i| argv[*i].len())?;
    Some((i, shell))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExecProfile {
    pub largest: usize,
    pub start: String,
    /// Sum of non-spillable words, including their NULs. No node-added bytes or pointers.
    pub total: usize,
}

impl ExecProfile {
    pub fn of(argv: &[String]) -> Option<Self> {
        if argv.is_empty() { return None; }
        // The node consumes an env prefix before detecting the interpreter's script.
        let command = super::loaders::command_of(argv);
        let spill = script_word(command).map(|(i, _)| argv.len() - command.len() + i);
        let mut profile = Self { largest: 0, start: String::new(), total: 0 };
        for (_, word) in argv.iter().enumerate().filter(|(i, _)| Some(*i) != spill) {
            let bytes = word.len() + 1;
            profile.total = profile.total.saturating_add(bytes);
            if bytes > profile.largest {
                profile.largest = bytes;
                profile.start = word.chars().take(40).collect();
            }
        }
        Some(profile)
    }

    pub fn over(&self, limits: ExecLimits) -> Option<String> {
        if self.total <= limits.total && limits.per_string.is_none_or(|one| self.largest <= one) { return None; }
        limits.word_over(&format!("the argument `{}...`", self.start), self.largest, 0)
            .or_else(|| limits.total_over(self.total)).map(|over| crate::os::exec_refusal(&over))
    }

    pub fn to_json(&self) -> String {
        format!("{{\"largest\":{},\"start\":{},\"total\":{}}}", self.largest, makepad_remote::json_str(&self.start), self.total)
    }

    pub fn parse(v: &makepad_strict_json::Value) -> Option<Self> {
        Some(Self { largest: usize::try_from(v.get("largest")?.as_u64()?).ok()?, start: v.get("start")?.as_str()?.chars().take(40).collect(), total: usize::try_from(v.get("total")?.as_u64()?).ok()? })
    }
}

/// A Windows run as the node starts it: plain argv, a program and its words, never a shell's
/// command line. A lane's `cmd /c ... | findstr` and `powershell -EncodedCommand` jobs on a
/// Windows node read to Defender, beside a crash's WerFault, as masquerade (11-06). cmd is
/// refused, and so is a batch file (Windows runs one under cmd). A PowerShell script given on the
/// command line (`-Command`, `-EncodedCommand`, or Windows PowerShell's positional command) is
/// written to `<stem>.ps1` and run with `-File`, under `-Command`'s exit: 1 when its last
/// statement failed (`-File` alone ends 0 then, and a failed last step would pass). A script file
/// runs under the box's execution policy, which may refuse every script (a client's default):
/// the run's own process takes RemoteSigned ([`POLICY_ENV`], the policy's process scope), which
/// runs an unsigned file written on the box and nothing downloaded, never `-ExecutionPolicy
/// Bypass` on its command line (a pattern Defender's behaviour rules watch). None: the argv is
/// plain as it is.
pub fn windows_plain(argv: &[String], stem: &Path) -> Result<Option<PlainRun>, String> {
    let Some(first) = argv.first() else { return Ok(None) };
    let file = first.rsplit(['\\', '/']).next().unwrap_or(first).to_ascii_lowercase();
    let name = file.strip_suffix(".exe").unwrap_or(&file);
    if name == "cmd" || file.ends_with(".bat") || file.ends_with(".cmd") {
        return Err(format!("`{first}`: a Windows run is plain argv, never cmd (a batch file runs under it): run the program itself, and a pipeline or redirect as a PowerShell script (`powershell -NoProfile -Command \"<script>\"`, which the node runs from a .ps1 file)"));
    }
    let windows_powershell = name == "powershell";
    if !windows_powershell && name != "pwsh" { return Ok(None); }
    let mut i = 1;
    let (at, body) = loop {
        let Some(word) = argv.get(i) else { return Ok(None) };
        let Some(key) = word.strip_prefix("--").or_else(|| word.strip_prefix(['-', '/'])).map(str::to_ascii_lowercase) else {
            // Windows PowerShell runs a positional word as its command, pwsh as its script file
            if !windows_powershell { return Ok(None); }
            break (i, argv[i..].join(" "));
        };
        let abbreviates = |full: &str, shortest: usize| key.len() >= shortest && full.starts_with(&key);
        if key == "ec" || abbreviates("encodedcommand", 1) {
            let text = argv.get(i + 1).ok_or_else(|| format!("`{word}` has no script after it"))?;
            if let Some(rest) = argv.get(i + 2) {
                return Err(format!("`{rest}` after the encoded script: the node runs the script from a file, so its options go before `{word}`"));
            }
            let utf16: Vec<u16> = super::exec::b64_decode(text).filter(|b| b.len() % 2 == 0).ok_or_else(|| format!("`{word}`: its script is no base64 of UTF-16LE text"))?
                .chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            break (i, String::from_utf16(&utf16).map_err(|_| format!("`{word}`: its script is no UTF-16LE text"))?);
        }
        if abbreviates("command", 1) {
            match argv.get(i + 1).map(String::as_str) {
                None => return Err(format!("`{word}` has no script after it")),
                Some("-") => return Err(format!("`{word} -` reads its script from stdin, and a run has none: give the script itself")),
                Some(_) => break (i, argv[i + 1..].join(" ")),
            }
        }
        if abbreviates("file", 1) { return Ok(None); }
        if key == "ea" || abbreviates("encodedarguments", 8) {
            return Err(format!("`{word}`: the node runs an encoded script from a file, without encoded arguments: put them in the script"));
        }
        // an option's value is no positional command
        let valued = [("executionpolicy", 2), ("ep", 2), ("windowstyle", 1), ("outputformat", 1), ("of", 2), ("inputformat", 2), ("if", 2), ("version", 1), ("psconsolefile", 3), ("configurationname", 6), ("custompipename", 2), ("workingdirectory", 2), ("wd", 2), ("settingsfile", 8)];
        i += if valued.iter().any(|&(full, shortest)| abbreviates(full, shortest)) { 2 } else { 1 };
    };
    let mut words = argv[..at].to_vec();
    // a policy the run names itself is its own
    let named = |w: &String| w.strip_prefix("--").or_else(|| w.strip_prefix(['-', '/'])).is_some_and(|k| { let k = k.to_ascii_lowercase(); k == "ep" || (k.len() >= 2 && "executionpolicy".starts_with(&k)) });
    let env = if words.iter().any(named) { Vec::new() } else { vec![(POLICY_ENV.to_string(), "RemoteSigned".to_string())] };
    let script = stem.with_extension("ps1");
    words.extend(["-File".to_string(), script.to_string_lossy().into_owned()]);
    // a BOM: Windows PowerShell reads a file without one in the box's ANSI code page
    let text = format!("\u{feff}{}\nif (-not $?) {{ exit 1 }}\n", body.trim_end());
    Ok(Some(PlainRun { argv: words, script, text, env }))
}

/// PowerShell's execution policy for its own process (and what it starts): the process scope,
/// which `-ExecutionPolicy` sets too.
pub const POLICY_ENV: &str = "PSExecutionPolicyPreference";

/// A Windows run's plain argv and the script file it runs ([`windows_plain`]).
#[derive(Debug, PartialEq)]
pub struct PlainRun {
    pub argv: Vec<String>,
    pub script: PathBuf,
    /// what the file holds
    pub text: String,
    /// what the run's environment gains (unless the run sets it itself)
    pub env: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn utf16_b64(script: &str) -> String {
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        makepad_network::digest::base64_encode(&bytes)
    }

    /// A Windows run never starts cmd, nor PowerShell with its script on the command line: a lane's
    /// `cmd /c` and `-EncodedCommand` jobs read to Defender as masquerade (11-06).
    #[test]
    fn a_windows_run_is_plain_argv() {
        let stem = Path::new("/lease/tmp/run-3-script");
        let ps1 = stem.with_extension("ps1").to_string_lossy().into_owned();
        let file = |script: &str| format!("\u{feff}{script}\nif (-not $?) {{ exit 1 }}\n");
        for refused in [&["cmd", "/c", "cargo check | findstr x"][..], &["C:\\Windows\\System32\\cmd.exe", "/k", "x"], &["CMD.EXE", "/c", "x"], &["build.bat"], &["tools\\x.CMD", "a"]] {
            let e = windows_plain(&words(refused), stem).expect_err(refused[0]);
            assert!(e.contains("never cmd") && e.contains("-Command"), "{e}");
        }
        for plain in [&["cargo", "check"][..], &["ping", "-n", "2", "127.0.0.1"], &["python", "-c", "print(1)"], &["powershell", "-NoProfile", "-File", "x.ps1", "-c", "a"], &["pwsh", "script.ps1", "-Command", "x"], &["pwsh", "-NoProfile"], &["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass"], &[]] {
            assert_eq!(windows_plain(&words(plain), stem), Ok(None), "{plain:?}");
        }
        let run = |argv: &[&str]| windows_plain(&words(argv), stem).unwrap_or_else(|e| panic!("{argv:?}: {e}")).unwrap_or_else(|| panic!("{argv:?} stayed as it was"));
        // the script, from every spelling of its option, runs from its file under -Command's exit,
        // and its process under RemoteSigned
        let bypass = |argv: &[&str], options: &[&str], script: &str| {
            let r = run(argv);
            let mut want = words(options);
            want.extend(words(&["-File", &ps1]));
            assert_eq!(r, PlainRun { argv: want, script: stem.with_extension("ps1"), text: file(script), env: vec![(POLICY_ENV.into(), "RemoteSigned".into())] }, "{argv:?}");
            assert!(!r.argv.iter().any(|w| w.to_ascii_lowercase().contains("bypass")));
        };
        bypass(&["powershell", "-NoProfile", "-Command", "cargo check | Select-String x\n"], &["powershell", "-NoProfile"], "cargo check | Select-String x");
        bypass(&["C:\\Program Files\\PowerShell\\7\\pwsh.exe", "-c", "Write-Output", "hi"], &["C:\\Program Files\\PowerShell\\7\\pwsh.exe"], "Write-Output hi");
        bypass(&["powershell", "/command", "exit 3"], &["powershell"], "exit 3");
        for option in ["-EncodedCommand", "-enc", "-ec", "-e", "/EncodedCommand", "--encodedcommand"] {
            bypass(&["powershell", "-NoProfile", "-NonInteractive", option, &utf16_b64("Write-Output 'h\u{e9}llo'; exit 4")], &["powershell", "-NoProfile", "-NonInteractive"], "Write-Output 'h\u{e9}llo'; exit 4");
        }
        // Windows PowerShell's positional command, past option values
        bypass(&["powershell", "-w", "hidden", "-Version", "5.1", "Get-Date;", "exit", "3"], &["powershell", "-w", "hidden", "-Version", "5.1"], "Get-Date; exit 3");
        // a policy of the run's own is kept, and its process gains none
        let r = run(&["powershell", "-ExecutionPolicy", "AllSigned", "-enc", &utf16_b64("x")]);
        assert_eq!((r.argv, r.text, r.env), (words(&["powershell", "-ExecutionPolicy", "AllSigned", "-File", &ps1]), file("x"), Vec::new()));
        let r = run(&["powershell", "-ep", "Unrestricted", "x"]);
        assert_eq!((r.argv, r.env), (words(&["powershell", "-ep", "Unrestricted", "-File", &ps1]), Vec::new()));
        // what has no file form is refused, and says why
        for (argv, why) in [
            (vec!["powershell", "-Command", "-"], "stdin"),
            (vec!["pwsh", "-NoProfile", "-Command"], "no script after it"),
            (vec!["powershell", "-enc"], "no script after it"),
            (vec!["powershell", "-enc", "a*b"], "no base64"),
            (vec!["powershell", "-enc", "QQ=="], "no base64"),
            (vec!["powershell", "-enc", &utf16_b64("x"), "-NoProfile"], "go before"),
            (vec!["powershell", "-ea", "AA==", "-enc", &utf16_b64("x")], "encoded arguments"),
        ] {
            let e = windows_plain(&words(&argv), stem).expect_err(&argv.join(" "));
            assert!(e.contains(why), "{argv:?}: {e}");
        }
    }
}
