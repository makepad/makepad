#!/usr/bin/env python3
"""The Windows Builder as source: the ZIP makepad.nl/builder/windows serves.

    python3 tools/makepad_builder/windows/package.py [--rev REV] --out makepad-builder.zip

The ZIP holds makepad-builder.bat and, under builder/source/, the source of
the Builder (a console program: its TUI runs in the .bat's own window) with
the few small crates of this repository it depends on and nothing from
crates.io. makepad-builder.bat downloads Rust's official GNU toolchain,
compiles that source and runs it; no program of ours ships in binary form.
Everything the Builder writes later goes in builder/ too, so the folder
itself only ever holds the .bat, makepad-builder.exe and the apps.

Takes the files of REV (default HEAD) of every package in `cargo tree -p
makepad-loader --target all`: what compiles (the manifest without its
dev-dependencies, benches, examples and tests; src/, build.rs, the files
the code includes) and the licences, nothing else. Path dependencies cargo
only reads (another platform's, optional ones) go in as their manifest with
empty targets. builder/source/Cargo.toml is a generated workspace naming
only the Builder, with a lock, so the build needs no network. The same REV
gives the same bytes. Also writes FILE.sha256.
"""
import argparse
import hashlib
import io
import os
import re
import subprocess
import zipfile
from pathlib import Path

def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--rev", default="HEAD")
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--package", default="makepad-loader", help="another package's closure (tests of the GNU chain)")
    parser.add_argument("--worktree", action="store_true", help="file contents from the working tree, not REV (tests of uncommitted changes)")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[3]
    commit = git(repo, "rev-parse", args.rev).decode().strip()

    # The packages, from the working tree's manifests (they must match REV).
    tree = subprocess.check_output(
        ["cargo", "tree", "-p", args.package, "--target", "all", "-e", "normal,build",
         "--prefix", "none", "--no-dedupe", "--offline"],
        cwd=repo, text=True)
    dirs = set()
    member = None
    for line in tree.splitlines():
        match = re.search(r"\((/[^)]*)\)", line)
        if not match:
            # A crates.io package of another platform (OpenHarmony's
            # hilog-sys): resolved from Cargo.lock, never compiled here.
            continue
        dirs.add(os.path.relpath(match.group(1), repo))
        if member is None:
            member = os.path.relpath(match.group(1), repo)
    manifests = [m for m in git(repo, "ls-tree", "-r", "--name-only", commit).decode().splitlines() if m.endswith("Cargo.toml")]
    packages = {os.path.dirname(m) for m in manifests}

    def owner(path):
        d = os.path.dirname(path)
        while d:
            if d in packages:
                return d
            d = os.path.dirname(d)
        return None

    # Cargo reads the manifest of every path dependency, optional and other
    # platforms' ones included, before it resolves features. Those it does
    # not build go in as their manifest and empty target files.
    import tomllib

    def manifest(d):
        if args.worktree:
            return tomllib.loads((repo / d / "Cargo.toml").read_text())
        return tomllib.loads(git(repo, "show", f"{commit}:{d}/Cargo.toml").decode())

    # Dev-dependencies are dropped from the manifests below.
    def path_deps(d, m):
        out = []
        tables = [m.get(k, {}) for k in ("dependencies", "build-dependencies")]
        for spec in m.get("target", {}).values():
            tables += [spec.get(k, {}) for k in ("dependencies", "build-dependencies")]
        for table in tables:
            for dep in table.values():
                if isinstance(dep, dict) and "path" in dep:
                    out.append(os.path.normpath(os.path.join(d, dep["path"])))
        return out

    stubs = {}
    todo = sorted(dirs)
    seen = set()
    while todo:
        d = todo.pop()
        if d in seen:
            continue
        seen.add(d)
        m = manifest(d)
        todo += [p for p in path_deps(d, m) if p in packages]
        if d not in dirs:
            targets = [m.get("lib", {}).get("path", "src/lib.rs")] if "lib" in m or not m.get("bin") else []
            for kind in ("bin",):
                targets += [t["path"] for t in m.get(kind, []) if "path" in t]
            build = m.get("package", {}).get("build")
            if build is None and subprocess.run(["git", "-C", str(repo), "cat-file", "-e", f"{commit}:{d}/build.rs"], stderr=subprocess.DEVNULL).returncode == 0:
                build = "build.rs"
            if isinstance(build, str):
                targets.append(build)
            stubs[d] = targets

    def text_of(path, blob):
        if args.worktree:
            return (repo / path).read_text(errors="replace")
        return git(repo, "cat-file", "blob", blob).decode(errors="replace")

    # What compiles: src/ (less a test module only `cfg(test)` declares),
    # build.rs, the manifest, and the files the code names (include_str!,
    # include_bytes!, include!, a resources/ path); and the licences.
    def wanted(path, package):
        rel = os.path.relpath(path, package)
        name = os.path.basename(rel)
        if rel == "Cargo.toml" or rel == "build.rs" or rel.startswith("src/"):
            return True
        return "/" not in rel and re.match(r"(LICEN[CS]E|COPYING|NOTICE)", name, re.I) is not None

    files = []
    listing = git(repo, "ls-tree", "-r", "-z", "--full-tree", commit, "--", *sorted(dirs)).split(b"\0")
    for entry in listing:
        if not entry:
            continue
        meta, path = entry.decode().split("\t", 1)
        mode, kind, blob = meta.split()
        if kind != "blob" or owner(path) not in dirs:
            continue
        if not wanted(path, owner(path)):
            continue
        if "/tests/" in path or path.endswith(".wast") or re.match(r"widgets/fonts/[^/]+\.ttf$", path):
            continue
        if args.worktree and not (repo / path).is_file():
            continue
        files.append((path, blob, mode))
    if args.worktree:
        # New files not yet committed (a test of uncommitted work).
        known = {path for path, _, _ in files}
        extra = git(repo, "ls-files", "-z", "--others", "--exclude-standard", "--", *sorted(dirs)).decode().split("\0")
        for path in extra:
            if path and path not in known and owner(path) in dirs and "/tests/" not in path and wanted(path, owner(path)):
                files.append((path, "worktree", "100644"))
    # Files a build script names (its icon): dep-info does not list them.
    build_resources = set()
    # The files the code names, and no test module only tests compile.
    everything = git(repo, "ls-tree", "-r", "-z", "--full-tree", "--name-only", commit, "--", *sorted(dirs)).decode().split("\0")
    by_path = {path: (blob, mode) for path, blob, mode in files}
    for path, blob, mode in list(files):
        if not path.endswith(".rs"):
            continue
        text = text_of(path, blob)
        package = owner(path)
        for literal in re.findall(r'include(?:_str|_bytes)?!\(\s*"([^"]+)"', text) + re.findall(r'"((?:\.\./)*resources/[^"]+)"', text):
            for base in (os.path.dirname(path), package):
                named = os.path.normpath(os.path.join(base, literal))
                if named in by_path or owner(named) != package:
                    continue
                if named in everything or (args.worktree and (repo / named).is_file()):
                    blob_id = "worktree" if args.worktree else git(repo, "rev-parse", f"{commit}:{named}").decode().strip()
                    by_path[named] = (blob_id, "100644")
                    files.append((named, blob_id, "100644"))
                if os.path.basename(path) == "build.rs" and named in by_path:
                    build_resources.add(named)
        if re.search(r"#\[cfg\(test\)\]\s*mod tests;", text):
            test = os.path.join(os.path.dirname(path), "tests.rs")
            files = [f for f in files if f[0] != test]
    for d, targets in stubs.items():
        blob = git(repo, "rev-parse", f"{commit}:{d}/Cargo.toml").decode().strip()
        files.append((f"{d}/Cargo.toml", blob, "100644"))
        for target in targets:
            files.append((os.path.normpath(os.path.join(d, target)), None, "100644"))  # empty: never compiled
    files.sort()

    # Only the Builder is a member: a path dependency inside a workspace's
    # directory would otherwise join it, and a member's every dependency,
    # optional and dev ones included, must resolve.
    others = sorted(d for d in set(dirs) | set(stubs) if d != member)
    root = open(repo / "Cargo.toml").read()
    def raw(path, blob):
        if args.worktree and blob:
            return (repo / path).read_bytes()
        return git(repo, "cat-file", "blob", blob) if blob else b""

    # Manifests of the packages that compile lose their dev-dependencies and
    # bench, example and test targets (none of those files ship).
    def content(path, blob):
        data = raw(path, blob)
        if os.path.basename(path) != "Cargo.toml" or os.path.dirname(path) not in dirs:
            return data
        out, skip = [], False
        for line in data.decode().splitlines(keepends=True):
            header = re.match(r"\s*\[\[?([^\]]+)\]", line)
            if header:
                table = header.group(1).strip()
                skip = (table in ("bench", "example", "test")
                        or re.search(r"(^|\.)dev-dependencies(\.|$)", table) is not None)
            if not skip:
                out.append(line)
        text = "".join(out)
        # No auto-discovered targets from folders that are not there.
        if "[package]" in text and "autobenches" not in text:
            text = text.replace("[package]\n", "[package]\nautobenches = false\nautoexamples = false\nautotests = false\n", 1)
        return text.encode()

    # The first line names the exact source (makepad-builder.bat compiles
    # again when it changes): the commit and a hash of every file's bytes.
    def workspace_for(files):
        tree = hashlib.sha256()
        for path, blob, _ in files:
            tree.update(path.encode() + b"\0" + content(path, blob) + b"\0")
        return (
        "# Generated by tools/makepad_builder/windows/package.py from makepad "
        + commit + " (source " + tree.hexdigest()[:16] + ").\n[workspace]\nmembers = [\"" + member + "\"]\nresolver = \"2\"\nexclude = [\n"
        + "".join(f"    \"{d}\",\n" for d in others) + "]\n\n"
        # The root's crates-io patches whose package is in this graph (a
        # crate that names windows-link by version still gets the one here).
        + "".join(
            ("[patch.crates-io]\n" if i == 0 else "") + line + "\n"
            for i, line in enumerate(
                line for line in re.findall(r'^(\S+ = \{ path = "([^"]+)" \})$', root, re.M) and [
                    m.group(0) for m in re.finditer(r'^\S+ = \{ path = "([^"]+)" \}$', root.split("[patch.crates-io]", 1)[-1].split("\n[", 1)[0], re.M)
                    if os.path.normpath(m.group(1)) in dirs
                ] or []
            )
        )
        + "\n[profile.release]\nincremental = false\n"
        )

    workspace = workspace_for(files)

    # The lock, from the generated workspace, so the build resolves nothing.
    import tempfile
    with tempfile.TemporaryDirectory() as scratch:
        for d in sorted(dirs):
            Path(scratch, d).mkdir(parents=True, exist_ok=True)
        for path, blob, mode in files:
            target = Path(scratch, path)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content(path, blob))
        Path(scratch, "Cargo.toml").write_text(workspace)
        # Start from the repository's lock (its versions are the ones this
        # tree builds with); cargo trims it to this workspace. An optional
        # dependency nothing enables and nothing has locked is then never
        # looked for (unicode-bidi names a ../smallvec that does not exist).
        if (repo / "Cargo.lock").is_file():
            Path(scratch, "Cargo.lock").write_bytes((repo / "Cargo.lock").read_bytes())
        subprocess.check_call(["cargo", "metadata", "--offline", "--format-version", "1"], cwd=scratch, stdout=subprocess.DEVNULL)
        lock = Path(scratch, "Cargo.lock").read_bytes()
        # Exactly the files a Windows build reads: compile it for Windows
        # here (a check, with the build scripts run) and take every source
        # file the compiler's dep-info names. Another platform's modules and
        # the files only they include stay out. Needs the
        # x86_64-pc-windows-gnu standard library (rustup target add).
        subprocess.check_call(
            ["cargo", "check", "--offline", "--locked", "--quiet", "--release", "--target", "x86_64-pc-windows-gnu",
             "-p", "makepad-loader", "--bin", "makepad-builder"],
            cwd=scratch, env={**os.environ, "CARGO_TARGET_DIR": str(Path(scratch, ".target"))})
        used = set()
        real = os.path.realpath(scratch)
        for dep_file in Path(scratch, ".target").rglob("*.d"):
            for word in re.split(r"(?<!\\)\s+", dep_file.read_text()):
                # Paths inside the workspace are relative to it.
                word = word.rstrip(":").replace("\\ ", " ")
                for prefix in (real + "/", scratch + "/"):
                    if word.startswith(prefix):
                        word = word[len(prefix):]
                if word and not os.path.isabs(word):
                    used.add(os.path.normpath(word))
    def ships(path):
        name = os.path.basename(path)
        return (path in used or name == "Cargo.toml" or name == "build.rs" or path in build_resources
                or re.match(r"(LICEN[CS]E|COPYING|NOTICE)", name, re.I) is not None)
    files = [f for f in files if ships(f[0])]
    workspace = workspace_for(files)

    bat = (Path(__file__).parent / "makepad-builder.bat").read_bytes()
    if not args.worktree:
        bat = git(repo, "show", f"{commit}:tools/makepad_builder/windows/makepad-builder.bat")
    # cmd.exe needs CRLF: with bare LF its labels and goto misparse and the
    # window closes at once. Git keeps the file with LF (eol=crlf applies to
    # checkouts only), so the ZIP always gets CRLF, whatever the source had.
    bat = bat.replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
    assert b"\r\r" not in bat
    data = io.BytesIO()
    with zipfile.ZipFile(data, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        def add(name, content):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            z.writestr(info, content)
        add("makepad-builder.bat", bat)
        add("builder/source/Cargo.toml", workspace.encode())
        add("builder/source/Cargo.lock", lock)
        for path, blob, mode in files:
            add("builder/source/" + path, content(path, blob))
    args.out.write_bytes(data.getvalue())
    digest = hashlib.sha256(data.getvalue()).hexdigest()
    Path(str(args.out) + ".sha256").write_text(f"{digest}  {args.out.name}\n")
    print(f"{args.out}: {len(files)} files, {len(dirs)} packages built, {len(stubs)} manifests only, {len(data.getvalue()) // 1024} KB, sha256 {digest}, makepad {commit[:9]}")


if __name__ == "__main__":
    main()
