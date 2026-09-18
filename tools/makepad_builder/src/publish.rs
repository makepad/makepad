//! Maintainer-side source packaging. Only the resulting packs/manifests reach the service.
use crate::{catalog, sha256};
use makepad_strict_json::{self as json, Value};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn git(
    repo: &Path,
    args: &[&str],
    input: Option<&[u8]>,
    index: Option<&Path>,
) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    cmd.env("GIT_AUTHOR_NAME", "Makepad Builder")
        .env("GIT_AUTHOR_EMAIL", "builder@makepad.nl")
        .env("GIT_COMMITTER_NAME", "Makepad Builder")
        .env("GIT_COMMITTER_EMAIL", "builder@makepad.nl");
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    if let Some(data) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(data)
            .map_err(|e| e.to_string())?;
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    Ok(out.stdout)
}
fn text(bytes: Vec<u8>) -> Result<String, String> {
    String::from_utf8(bytes)
        .map(|s| s.trim().into())
        .map_err(|e| e.to_string())
}
fn pack(repo: &Path, name: &str, path: &str, out: &Path, snapshot: bool) -> Result<Value, String> {
    let base = text(git(repo, &["rev-parse", "HEAD"], None, None)?)?;
    let commit = if snapshot {
        let index = out.join(format!(".{name}.index"));
        git(repo, &["read-tree", "HEAD"], None, Some(&index))?;
        git(repo, &["add", "--update", "--", "."], None, Some(&index))?;
        let extra = git(
            repo,
            &["ls-files", "--others", "--exclude-standard", "-z"],
            None,
            None,
        )?;
        let mut paths = Vec::new();
        for p in extra.split(|b| *b == 0).filter(|p| !p.is_empty()) {
            let s = std::str::from_utf8(p).map_err(|e| e.to_string())?;
            if (s.starts_with("src/") || s.starts_with("deps/"))
                && (s.ends_with(".rs") || s.ends_with("Cargo.toml") || s.ends_with("Cargo.lock"))
            {
                paths.extend_from_slice(p);
                paths.push(0);
            }
        }
        if !paths.is_empty() {
            git(
                repo,
                &["add", "--pathspec-from-file=-", "--pathspec-file-nul"],
                Some(&paths),
                Some(&index),
            )?;
        }
        let tree = text(git(repo, &["write-tree"], None, Some(&index))?)?;
        let commit = text(git(
            repo,
            &["commit-tree", &tree],
            Some(format!("Builder source snapshot of {base}\n").as_bytes()),
            None,
        )?)?;
        let _ = fs::remove_file(index);
        commit
    } else {
        base.clone()
    };
    let tree = text(git(
        repo,
        &["rev-parse", &format!("{commit}^{{tree}}")],
        None,
        None,
    )?)?;
    let listing = git(repo, &["ls-tree", "-r", "-t", "-z", &commit], None, None)?;
    let mut objects = std::collections::BTreeSet::from([commit.clone(), tree]);
    for row in listing.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let fields = std::str::from_utf8(row.split(|b| *b == b'\t').next().unwrap())
            .map_err(|e| e.to_string())?;
        let mut parts = fields.split_whitespace();
        let mode = parts.next();
        let _kind = parts.next();
        let oid = parts.next().ok_or("Invalid Git tree record")?;
        if mode == Some("160000") {
            return Err(
                "Source release has a submodule; include its pinned sources explicitly".into(),
            );
        }
        objects.insert(oid.into());
    }
    let input = objects.into_iter().collect::<Vec<_>>().join("\n") + "\n";
    let data = git(
        repo,
        &["pack-objects", "--stdout"],
        Some(input.as_bytes()),
        None,
    )?;
    fs::write(out.join(format!("{name}.pack")), &data).map_err(|e| e.to_string())?;
    println!("{name}: commit={commit} base={base} bytes={}", data.len());
    Ok(json::obj(vec![
        ("name", json::s(name)),
        ("path", json::s(path)),
        ("commit", json::s(commit)),
        ("sha256", json::s(sha256::sha256_hex(&data))),
        ("bytes", Value::Int(data.len() as i64)),
    ]))
}
pub fn main() -> Result<(), String> {
    let mut fields = std::collections::HashMap::new();
    let mut snapshot = false;
    let mut cuda = false;
    let mut args = env::args().skip(2);
    while let Some(arg) = args.next() {
        if arg == "--snapshot" {
            snapshot = true;
        } else if arg == "--cuda" {
            cuda = true;
        } else {
            fields.insert(arg, args.next().ok_or("Missing publisher argument")?);
        }
    }
    let get = |s: &str| fields.get(s).cloned().ok_or_else(|| format!("Missing {s}"));
    let app = get("--app-id")?;
    let release = get("--release")?;
    if !catalog::identifier(&app) || !catalog::identifier(&release) {
        return Err("Invalid app/release ID".into());
    }
    let root = PathBuf::from(get("--out")?);
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let dest = root.join("releases").join(&app).join(&release);
    if dest.exists() {
        return Err("Release ID already exists; releases are immutable".into());
    }
    fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    let makepad = pack(
        Path::new(&get("--makepad")?),
        "makepad",
        "makepad",
        &dest,
        snapshot,
    )?;
    let app_repo = pack(
        Path::new(&get("--app-repo")?),
        &app,
        &format!("makepad/apps/{app}"),
        &dest,
        snapshot,
    )?;
    let manifest = json::obj(vec![
        ("id", json::s(&app)),
        ("title", json::s(get("--title")?)),
        ("release", json::s(&release)),
        (
            "rust",
            json::s(
                fields
                    .get("--rust")
                    .cloned()
                    .unwrap_or_else(|| crate::rustc::DEFAULT_VERSION.into()),
            ),
        ),
        ("package", json::s(get("--package")?)),
        ("binary", json::s(get("--binary")?)),
        ("workspace", json::s(format!("makepad/apps/{app}"))),
        (
            "platforms",
            Value::Arr(
                [
                    "aarch64-apple-darwin",
                    "x86_64-apple-darwin",
                    "x86_64-pc-windows-msvc",
                    "x86_64-unknown-linux-gnu",
                ]
                .into_iter()
                .map(json::s)
                .collect(),
            ),
        ),
        ("cuda", Value::Bool(cuda)),
        ("repositories", Value::Arr(vec![makepad, app_repo])),
    ]);
    catalog::Release::parse(&manifest)?;
    let manifest = manifest.to_json();
    fs::write(dest.join("release.json"), &manifest).map_err(|e| e.to_string())?;
    let app_dir = root.join("apps").join(app);
    fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;
    let tmp = app_dir.join("latest.json.new");
    fs::write(&tmp, manifest).map_err(|e| e.to_string())?;
    fs::rename(tmp, app_dir.join("latest.json")).map_err(|e| e.to_string())?;
    println!("Published manifest {}", dest.display());
    Ok(())
}
