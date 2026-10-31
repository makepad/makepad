#!/usr/bin/env python3
"""Select the quick gate from both pinned diffs; run on a leased node only.

A gate checks and presents what the diff reaches (user 2026-10-30, landing design):
a change inside one app gates that app. The SMOKE apps all go when the diff reaches
the JIT itself (makepad-run's dependencies; the devhub linked into its binary does
not count, it changes no app's code) or a workspace-wide input; the other
GATE_PACKAGES are checked when the diff reaches them. A diff that reaches no gate
package gates Calculator, the one canary. Prints `jit yes|no`: whether the diff
touches what the JIT binary is built from.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

# the smoke set, in run order: gated when the diff reaches the JIT or a workspace-wide input
SMOKE = ("makepad-stage", "makepad-scope", "makepad-sandbox", "makepad-fab", "makepad-app-synth", "makepad-app-calculator")
# checked too when the diff reaches them
GATE_PACKAGES = SMOKE + ("makepad-example-counter", "makepad-platform", "makepad-draw", "makepad-widgets")
CALC = "makepad-app-calculator"
# the JIT binary, and what it links that changes no app's code
JIT_PACKAGE = "makepad-run"
NOT_APP_CODE = ("makepad-devhub",)
# Explicit launch recipes: being a Cargo binary alone does not imply a present.
PRESENT_ARGS = {
    "makepad-stage": [],
    "makepad-scope": [],
    "makepad-sandbox": [],
    "makepad-fab": [],
    "makepad-app-synth": [],
    CALC: ["--no-default-features", "--features", "standalone"],
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

    # the gate's own scripts and docs compile into nothing
    compiled = [p for p in files if not p.startswith("tools/run/fixture/") and not p.endswith(".md")]
    touched = {owner(path) for path in compiled} - {None}
    affected = closure(touched, reverse)
    by_name = {p["name"]: d for d, p in packages.items()}
    missing = [name for name in SMOKE if name not in by_name]
    if missing:
        raise RuntimeError("smoke packages missing from metadata: " + " ".join(missing))

    # What the JIT binary is built from, with and without what changes no app's code.
    forward = {}
    for directory, pkg in packages.items():
        forward[directory] = {Path(d["path"]).resolve() for d in pkg["dependencies"]
                              if d.get("path") and d["kind"] != "dev"}
    jit_dir = by_name.get(JIT_PACKAGE)
    cut = {by_name[n] for n in NOT_APP_CODE if n in by_name}
    binary = closure({jit_dir}, forward) if jit_dir else set()
    jit_code = closure({jit_dir}, {d: e - cut for d, e in forward.items()}) if jit_dir else set()
    # Workspace-wide resolver/patch/profile inputs apply to every member.
    workspace = any(p in ("Cargo.toml", "Cargo.lock", ".cargo/config", ".cargo/config.toml") for p in compiled)
    if workspace:
        affected.update(packages)
    selected = {name for name in GATE_PACKAGES if by_name.get(name) in affected}
    if workspace or touched & jit_code:
        selected |= set(SMOKE)
    if not selected:
        selected = {CALC}

    def where(name):
        # Launch from the actual package directory; works for nested workspaces.
        return str(by_name[name].relative_to(root))

    print(f"why files {len(files)}; changed: {' '.join(sorted(packages[d]['name'] for d in touched)) or '-'}; affected gate packages: {' '.join(sorted(selected)) or '-'}")
    print("jit " + ("yes" if workspace or touched & binary else "no"))
    for name in GATE_PACKAGES:
        if name in selected:
            print(f"check {name} {where(name)}")
    for name in SMOKE:
        if name in PRESENT_ARGS and name in selected:
            print(" ".join(["present", name, where(name), *PRESENT_ARGS[name]]))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
