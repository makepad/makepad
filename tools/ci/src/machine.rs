//! Explicit fleet operations through the existing tunnel client. No implicit installs.
use crate::{
    cargo::{self, CargoResult, Options},
    process::{Output, Result},
    report::{strings, Run},
};
use std::path::PathBuf;
#[derive(Clone, Debug)]
pub struct Machine {
    pub tunnel: String,
    pub root: String,
    pub os: String,
}
fn posix(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}
fn powershell(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
fn utf16_base64(text: &str) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<_> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}
// The tunnel daemon owns one foreground command. Serialize commands per
// endpoint across script workers so one test cannot replace another's child.
fn tunnel_lock(address: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::BTreeMap<String, std::sync::Arc<std::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    LOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(address.into())
        .or_default()
        .clone()
}
impl Machine {
    pub fn shell_args(
        &self,
        program: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Vec<String>> {
        if program.is_empty() || program.contains('\0') || args.iter().any(|a| a.contains('\0')) {
            return Err("invalid remote command".into());
        }
        if env.iter().any(|(k, _)| {
            k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }) {
            return Err("invalid environment name".into());
        }
        let mut env = env.to_vec();
        env.retain(|(key, _)| key != "CARGO_TARGET_DIR");
        env.push(("CARGO_TARGET_DIR".into(), format!("{}/target", self.root.trim_end_matches(['/', '\\']))));
        let shell = if self.os == "windows" {
            let assignments = env
                .iter()
                .map(|(k, v)| format!("$env:{k}={}; ", powershell(v)))
                .collect::<String>();
            let script=format!("$ErrorActionPreference='Stop'; Set-Location -LiteralPath {}; {assignments}& {} {}; exit $LASTEXITCODE",powershell(&self.root),powershell(program),args.iter().map(|a|powershell(a)).collect::<Vec<_>>().join(" "));
            format!(
                "powershell -NoProfile -NonInteractive -EncodedCommand {}",
                utf16_base64(&script)
            )
        } else {
            let assignments = env
                .iter()
                .map(|(k, v)| format!("{k}={}", posix(v)))
                .collect::<Vec<_>>()
                .join(" ");
            format!(
                "cd {} && {}{} {} {}",
                posix(&self.root),
                if assignments.is_empty() { "" } else { "env " },
                assignments,
                posix(program),
                args.iter().map(|a| posix(a)).collect::<Vec<_>>().join(" ")
            )
        };
        Ok(vec![
            "tunnel".into(),
            self.tunnel.clone(),
            "--no-sync".into(),
            "shell".into(),
            shell,
        ])
    }
    pub fn push_args(&self, local: &std::path::Path, relative: &str) -> Result<Vec<String>> {
        if relative.starts_with(['/', '\\'])
            || relative.contains(':')
            || relative.split(['/', '\\']).any(|p| p == "..")
        {
            return Err("tunnel push destination must be relative".into());
        }
        Ok(vec![
            "tunnel".into(),
            self.tunnel.clone(),
            "push".into(),
            local.display().to_string(),
            relative.into(),
        ])
    }
    fn client(run: &Run) -> Result<PathBuf> {
        let p = run
            .root
            .join("target/release")
            .join(format!("cargo-makepad{}", std::env::consts::EXE_SUFFIX));
        if !p.is_file() {
            return Err(format!(
                "tunnel client missing: {}; build makepad cargo tool first",
                p.display()
            ));
        }
        Ok(p)
    }
    pub fn run(
        &self,
        run: &mut Run,
        program: &str,
        args: &[String],
        env: &[(String, String)],
        timeout: u64,
    ) -> Result<Output> {
        let lock = tunnel_lock(&self.tunnel);
        let _guard = lock.lock().map_err(|e| e.to_string())?;
        let client = Self::client(run)?;
        let args = self.shell_args(program, args, env)?;
        let root = run.root.clone();
        if program == "cargo" {
            run.cargo_command(&client.display().to_string(), &args, &root, &[], timeout).map(|r| r.output)
        } else {
            run.command(&client.display().to_string(), &args, &root, &[], timeout)
        }
    }
    pub fn cargo(&self, run: &mut Run, args: &[String], opts: &Options) -> Result<CargoResult> {
        let probe = self.run(run, "rustc", &strings(&["-vV"]), &[], 30)?;
        if probe.code != 0 {
            return Err(crate::process::error_lines(&probe.out));
        }
        let host = probe
            .out
            .lines()
            .find_map(|s| s.strip_prefix("host: "))
            .ok_or("remote rustc missing host target")?;
        let args = cargo::command_args(args, opts, host)?;
        let mut env = opts.env.clone();
        env.retain(|(k, _)| k != "CARGO_TARGET_DIR");
        env.push((
            "CARGO_TARGET_DIR".into(),
            format!("{}/target", self.root.trim_end_matches(['/', '\\'])),
        ));
        env.push(("CARGO_TERM_COLOR".into(), "never".into()));
        let (program, args) = if self.os == "windows" {
            ("cargo", args)
        } else {
            (
                "nice",
                std::iter::once("cargo".into()).chain(args).collect(),
            )
        };
        let lock = tunnel_lock(&self.tunnel);
        let _guard = lock.lock().map_err(|e| e.to_string())?;
        let client = Self::client(run)?;
        let args = self.shell_args(program, &args, &env)?;
        let root = run.root.clone();
        run.cargo_command(&client.display().to_string(), &args, &root, &[],
            if opts.timeout == 0 { 3600 } else { opts.timeout })
    }

    pub fn sync(&self, run: &mut Run) -> Result<()> {
        let lock = tunnel_lock(&self.tunnel);
        let _guard = lock.lock().map_err(|e| e.to_string())?;
        if !matches!(run.tip.len(), 40 | 64) || !run.tip.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(
                "machine sync requires a committed run tip (use --once, or commit before --run)"
                    .into(),
            );
        }
        // HEAD names the exact detached run revision. No private branch is pushed.
        let root = run.root.clone();
        let head = run.command("git", &strings(&["rev-parse", "HEAD"]), &root, &[], 30)?;
        if head.code != 0 || head.out.trim() != run.tip {
            return Err("checkout HEAD differs from the run tip".into());
        }
        let file = run.path("machine.bundle");
        let tip = run.tip.clone();
        let output = run.command(
            "git",
            &strings(&["bundle", "create", &file.display().to_string(), "HEAD"]),
            &root,
            &[],
            300,
        )?;
        if output.code != 0 {
            return Err(crate::process::error_lines(&output.out));
        }
        if std::fs::metadata(&file).map_err(|e| e.to_string())?.len() >= 2_000_000_000 {
            return Err("git bundle exceeds tunnel push limit".into());
        }
        let relative = format!("local/ci/bundles/{tip}-{}.bundle", std::process::id());
        let client = Self::client(run)?;
        let output = run.command(
            &client.display().to_string(),
            &self.push_args(&file, &relative)?,
            &root,
            &[],
            300,
        )?;
        if output.code != 0 {
            return Err(crate::process::error_lines(&output.out));
        }
        // push paths are relative to the daemon's working directory, not machine.root.
        // Resolve that path before changing directory; fetch/checkout target only root.
        let shell = if self.os == "windows" {
            let script=format!("$ErrorActionPreference='Stop'; $bundle=(Join-Path (Get-Location) {}); Set-Location -LiteralPath {}; git fetch $bundle HEAD; if ($LASTEXITCODE -ne 0) {{exit $LASTEXITCODE}}; git checkout --detach {}; exit $LASTEXITCODE",powershell(&relative),powershell(&self.root),powershell(&tip));
            format!(
                "powershell -NoProfile -NonInteractive -EncodedCommand {}",
                utf16_base64(&script)
            )
        } else {
            format!("ci_bundle=\"$PWD\"/{}; cd {} && git fetch \"$ci_bundle\" HEAD && git checkout --detach {}",posix(&relative),posix(&self.root),posix(&tip))
        };
        let args = vec![
            "tunnel".into(),
            self.tunnel.clone(),
            "--no-sync".into(),
            "shell".into(),
            shell,
        ];
        let output = run.command(&client.display().to_string(), &args, &root, &[], 300)?;
        if output.code != 0 {
            return Err(crate::process::error_lines(&output.out));
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tunnel_commands_quote_paths_and_use_no_sync() {
        let m = Machine {
            tunnel: "box:8384".into(),
            root: "/repo with ' quote".into(),
            os: "linux".into(),
        };
        let a = m
            .shell_args("printf", &strings(&["$(touch nope)", "a b"]), &[])
            .unwrap();
        assert_eq!(
            &a[..4],
            &strings(&["tunnel", "box:8384", "--no-sync", "shell"])
        );
        assert!(a[4].contains("'$(touch nope)'"));
        assert!(m.push_args(std::path::Path::new("a"), "/absolute").is_err());
        assert!(m.push_args(std::path::Path::new("a"), "../escape").is_err());
        assert!(m
            .push_args(std::path::Path::new("a"), "local/ci/a.bundle")
            .is_ok());
    }
    #[test]
    fn windows_shell_uses_encoded_powershell() {
        let m = Machine {
            tunnel: "box:8384".into(),
            root: "C:\\CI checkout".into(),
            os: "windows".into(),
        };
        let a = m.shell_args("cargo", &strings(&["test"]), &[]).unwrap();
        assert!(a[4].starts_with("powershell -NoProfile -NonInteractive -EncodedCommand "));
        assert!(!a[4].contains("C:"));
        assert_eq!(utf16_base64("A"), "QQA=");
    }
}
