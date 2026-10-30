#!/usr/bin/env python3
"""Select the quick gate from both pinned diffs; run on a leased node only.

Checks are restricted to GATE_PACKAGES plus Calculator. Presents are restricted
to affected apps with a supported hidden JIT launch (Calculator is check-only
when it is just the smoke dependency). No size caps or unrelated Stage fallback.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

GATE_PACKAGES = (
    "makepad-example-counter", "makepad-app-calculator", "makepad-platform",
    "makepad-draw", "makepad-widgets", "makepad-app-wm-all", "makepad-stage",
    "makepad-fab", "makepad-app-synth", "makepad-sandbox",
)
CALC = "makepad-app-calculator"
# Explicit launch recipes: being a Cargo binary alone does not imply a present.
PRESENT_ARGS = {
    "makepad-example-counter": [],
    CALC: ["--no-default-features", "--features", "standalone"],
    "makepad-app-wm-all": [],
    "makepad-stage": [],
    "makepad-fab": [],
    "makepad-app-synth": [],
    "makepad-sandbox": [],
}


def git(cwd, *args):
    return subprocess.check_output(["git", "-C", str(cwd), *args]).decode()


def metadata(manifest):
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--offline",
         "--manifest-path", str(manifest)], capture_output=True, text=True,
    )
    if result.returncode:
        raise RuntimeError(result.stderr)
    return json.loads(result.stdout)


def closure(start, edges):
    seen, todo = set(start), list(start)
    while todo:
        for node in edges.get(todo.pop(), ()):
            if node not in seen:
                seen.add(node)
                todo.append(node)
    return seen


def main():
    root = Path.cwd().resolve()
    base, cbase = sys.argv[1:3]
    # --no-renames includes both the removed and added path; NUL keeps spaces safe.
    files = [p for p in git(root, "diff", "--no-renames", "--name-only", "-z", base, "HEAD").split("\0") if p]
    commercial = root / "apps/commercial"
    if cbase != "-":
        if not (commercial / ".git").exists():
            raise RuntimeError("commercial base supplied but checkout is missing")
        files += ["apps/commercial/" + p for p in git(commercial, "diff", "--no-renames", "--name-only", "-z", cbase, "HEAD").split("\0") if p]

    # Key by manifest directory, not Cargo package ID: separate workspaces can
    # assign different IDs to the same path dependency. Keep excluded path libs
    # too, so changes there propagate through the entire dependency chain.
    packages = {}

    def load(manifest):
        for pkg in metadata(manifest)["packages"]:
            directory = Path(pkg["manifest_path"]).resolve().parent
            if pkg["source"] is None and root in directory.parents:
                packages[directory] = pkg

    load(root / "Cargo.toml")
    # Older commercial tips keep Sandbox outside the enclosing workspace.
    sandbox = commercial / "sandbox/Cargo.toml"
    if sandbox.exists() and not any(p["name"] == "makepad-sandbox" for p in packages.values()):
        load(sandbox)

    reverse = {}
    for directory, pkg in packages.items():
        # Use declared path dependencies, including optional and target-specific
        # edges; cargo's host/default resolve graph can hide affected consumers.
        for dep in pkg["dependencies"]:
            if dep.get("path") and dep["kind"] != "dev":
                reverse.setdefault(Path(dep["path"]).resolve(), set()).add(directory)

    def owner(path):
        path = root / path
        return next((parent for parent in (path.parent, *path.parents)
                     if parent in packages), None)

    touched = {owner(path) for path in files} - {None}
    affected = closure(touched, reverse)
    # Workspace-wide resolver/patch/profile inputs apply to every member.
    if any(p in ("Cargo.toml", "Cargo.lock", ".cargo/config", ".cargo/config.toml") for p in files):
        affected.update(packages)
    by_name = {p["name"]: d for d, p in packages.items()}
    if CALC not in by_name:
        raise RuntimeError("Calculator smoke package is missing from metadata")
    selected = {name for name in GATE_PACKAGES if by_name.get(name) in affected}

    def where(name):
        # Launch from the actual package directory; works for nested workspaces.
        return str(by_name[name].relative_to(root))

    print(f"why files {len(files)}; changed: {' '.join(sorted(packages[d]['name'] for d in touched)) or '-'}; affected gate packages: {' '.join(sorted(selected)) or '-'}")
    for name in GATE_PACKAGES:
        if name in selected or name == CALC:
            print(f"check {name} {where(name)}")
    for name in GATE_PACKAGES:
        if name in selected and name in PRESENT_ARGS:
            print(" ".join(["present", name, where(name), *PRESENT_ARGS[name]]))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
