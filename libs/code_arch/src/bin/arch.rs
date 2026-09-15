use makepad_code_arch::{changes_since, format, parse, try_manifest, validate, Limits, Plan};
use std::{
    env,
    fmt::Write as _,
    fs::{self, File},
    io::{self, Read, Write as _},
    path::{Path, PathBuf},
    process::ExitCode,
};

const USAGE: &str = "Usage:
  arch manifest <scope> [--root <repo>]
  arch validate <plan.toml> [--root <repo>]
  arch changes <plan.toml> [--root <repo>]
  arch format <plan.toml> [--write]

The repository root defaults to the current directory.
Plan filenames are relative to the current directory, independent of --root.
Exit codes: 0 success (including warnings or changes), 1 data/IO error, 2 usage error.
";

struct Command {
    name: String,
    input: PathBuf,
    root: PathBuf,
    write: bool,
}

fn arguments() -> Result<Option<Command>, String> {
    let mut args = env::args_os().skip(1);
    let name = args.next().ok_or("missing command")?;
    if name == "--help" || name == "-h" {
        return if args.next().is_none() {
            Ok(None)
        } else {
            Err("unexpected arguments after --help".into())
        };
    }
    let name = name.to_str().ok_or("command must be UTF-8")?;
    if !matches!(name, "manifest" | "validate" | "changes" | "format") {
        return Err(format!("unknown command `{name}`"));
    }
    let mut input = None;
    let mut root = None;
    let mut write = false;
    let mut positional = false;
    while let Some(arg) = args.next() {
        if !positional && arg == "--" {
            positional = true;
        } else if !positional && arg == "--root" && name != "format" {
            if root.is_some() {
                return Err("duplicate --root".into());
            }
            let value = args.next().ok_or("--root requires a repository path")?;
            if value.is_empty() || value.to_string_lossy().starts_with("--") {
                return Err("--root requires a repository path".into());
            }
            root = Some(PathBuf::from(value));
        } else if !positional && arg == "--write" && name == "format" {
            if write {
                return Err("duplicate --write".into());
            }
            write = true;
        } else if !positional && arg.to_string_lossy().starts_with('-') {
            return Err(format!(
                "unsupported option `{}` for {name}",
                arg.to_string_lossy()
            ));
        } else if arg.is_empty() || input.replace(PathBuf::from(arg)).is_some() {
            return Err("expected exactly one scope or plan filename".into());
        }
    }
    Ok(Some(Command {
        name: name.into(),
        input: input.ok_or("missing scope or plan filename")?,
        root: root.unwrap_or_else(|| PathBuf::from(".")),
        write,
    }))
}

fn read_plan(path: &Path) -> Result<Plan, String> {
    let limits = Limits::default();
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| {
            file.take(limits.max_input_bytes as u64 + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > limits.max_input_bytes {
        return Err(format!(
            "{}: input exceeds {} bytes",
            path.display(),
            limits.max_input_bytes
        ));
    }
    let text =
        std::str::from_utf8(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    parse(text, &limits).map_err(|error| format!("{}: {error}", path.display()))
}

// TOML basic strings, shared by the manifest and freshness arrays.
fn quoted(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ch if ch.is_control() => write!(out, "\\u{:04X}", ch as u32).unwrap(),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn array(values: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let values: Vec<_> = values
        .into_iter()
        .map(|value| quoted(value.as_ref()))
        .collect();
    format!("[{}]", values.join(", "))
}

fn stdout(text: &str) -> Result<(), String> {
    io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(|error| error.to_string())
}

fn run(command: Command) -> Result<(), String> {
    if command.name == "manifest" {
        let files =
            try_manifest(&command.input, &command.root).map_err(|error| error.to_string())?;
        let mut out = String::from("files = [\n");
        for file in files {
            let path = file.path.to_str().ok_or("manifest path is not UTF-8")?;
            writeln!(
                out,
                "    {{ path = {}, hash = {} }},",
                quoted(path),
                quoted(&file.hash)
            )
            .unwrap();
        }
        out.push_str("]\n");
        return stdout(&out);
    }
    let plan = read_plan(&command.input)?;
    match command.name.as_str() {
        "validate" => {
            let result = validate(&plan, &command.root);
            for error in &result.errors {
                eprintln!("error: {error}");
            }
            for warning in &result.warnings {
                eprintln!("warning: {}: {}", warning.field, warning.message);
            }
            stdout(&format!(
                "validation: {} errors, {} warnings\n",
                result.errors.len(),
                result.warnings.len()
            ))?;
            if !result.is_valid() {
                return Err("validation failed".into());
            }
        }
        "changes" => {
            let result = changes_since(&plan, &command.root);
            let mut out = String::new();
            for (name, paths) in [
                ("added", &result.added),
                ("removed", &result.removed),
                ("modified", &result.modified),
            ] {
                let paths = paths
                    .iter()
                    .map(|path| path.to_str().ok_or("changed path is not UTF-8"))
                    .collect::<Result<Vec<_>, _>>()?;
                writeln!(out, "{name} = {}", array(paths)).unwrap();
            }
            writeln!(
                out,
                "affected_nodes = {}\nunchanged = {}",
                array(&result.affected_nodes),
                result.unchanged
            )
            .unwrap();
            stdout(&out)?;
            for error in &result.errors {
                eprintln!("error: {error}");
            }
            if !result.errors.is_empty() {
                return Err("freshness check failed".into());
            }
        }
        "format" => {
            let text = format(&plan);
            if command.write {
                fs::write(&command.input, text)
                    .map_err(|error| format!("{}: {error}", command.input.display()))?;
            } else {
                stdout(&text)?;
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}

fn main() -> ExitCode {
    match arguments() {
        Ok(command) => match command.map_or_else(|| stdout(USAGE), run) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(1)
            }
        },
        Err(error) => {
            eprintln!("error: {error}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
