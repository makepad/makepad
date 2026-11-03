#!/usr/bin/env python3
"""Select the quick gate from both pinned diffs; run on a leased node only.

A gate checks and presents what the diff reaches (user 2026-10-30, landing design):
a change inside one app gates that app. The SMOKE apps all go when the diff reaches
the JIT itself (makepad-run's dependencies) or a workspace-wide input; the other
GATE_PACKAGES are checked when the diff reaches them, along with every changed
package with a lib or bin target. A diff that reaches no gate package gates
Calculator, the one canary; a diff only in devhub's own crate gates
its build alone. Prints `jit yes|no`: whether the diff touches what the JIT binary
is built from; `devhub yes|no`: whether it touches devhub's own binary. A `check` line may end in
`--also=<libs>`: libs checked as that build compiles them (feature flips, below).
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
# the JIT binary; devhub's binary
JIT_PACKAGE = "makepad-run"
DEVHUB_PACKAGE = "makepad-devhub"
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


def old_dep_features(repo, ref, rel, directory):
    """The features the manifest at `ref` turned on per path dependency (normal dependencies,
    every target): {dependency dir: features}; empty when the manifest is new."""
    import tomllib  # Python 3.11; the hot Mac's python3 is 3.9 and its mode never reads old manifests
    try:
        text = git(repo, "show", f"{ref}:{rel}")
    except subprocess.CalledProcessError:
        return {}
    manifest = tomllib.loads(text)
    tables = [manifest.get("dependencies", {})]
    tables += [t.get("dependencies", {}) for t in manifest.get("target", {}).values()]
    out = {}
    for table in tables:
        for dep in table.values():
            if isinstance(dep, dict) and "path" in dep:
                crate = (directory / dep["path"]).resolve()
                out[crate] = out.get(crate, frozenset()) | frozenset(dep.get("features", ()))
    return out


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

    # What the JIT binary and devhub's are built from.
    forward = {}
    for directory, pkg in packages.items():
        forward[directory] = {Path(d["path"]).resolve() for d in pkg["dependencies"]
                              if d.get("path") and d["kind"] != "dev"}
    jit_dir = by_name.get(JIT_PACKAGE)
    jit_code = closure({jit_dir}, forward) if jit_dir else set()
    devhub_dir = by_name.get(DEVHUB_PACKAGE)
    devhub_code = closure({devhub_dir}, forward) if devhub_dir else set()
    # Workspace-wide resolver/patch/profile inputs apply to every member.
    workspace = any(p in ("Cargo.toml", "Cargo.lock", ".cargo/config", ".cargo/config.toml") for p in compiled)
    if os.environ.get("HOT") == "1":
        # every land boots one small drawing app under the JIT to its first present: a broken draw
        # path (a shader IR change) fails every app, whichever crate the landing touched
        selected = {CALC}
        for name in SMOKE:
            if name not in PRESENT_ARGS:
                continue
            parts = by_name[name].relative_to(root).parts
            app = "/".join(parts[:3] if parts[:2] == ("apps", "commercial") else parts[:2])
            if parts[0] == "apps" and any(p.startswith(app + "/") for p in compiled):
                selected.add(name)
        print(f"why files {len(files)}; changed: {' '.join(sorted(packages[d]['name'] for d in touched)) or '-'}; affected gate packages: {' '.join(sorted(selected)) or '-'}; feature flips: -")
        print("jit " + ("yes" if workspace or touched & jit_code else "no"))
        print("devhub " + ("yes" if workspace or touched & devhub_code else "no"))
        for name in SMOKE:
            if name in selected:
                print(" ".join(["present", name, str(by_name[name].relative_to(root)), *PRESENT_ARGS[name]]))
        return
    if workspace:
        affected.update(packages)
    selected = {name for name in GATE_PACKAGES if by_name.get(name) in affected}
    if workspace or touched & jit_code:
        selected |= set(SMOKE)
    # devhub's crate alone reaches no app's code (the WM's and hub's links to it are not smoke apps)
    devhub_only = touched == {devhub_dir} and not workspace
    if not selected and not devhub_only:
        selected = {CALC}

    # Feature flips: features unify per build, so a package that turns on features of a crate
    # the changed code uses (Sandbox's sim turns on box3d's `double-precision`) changes how that
    # code compiles in every build that also holds the package (csg_mech's `t.p` became f64,
    # #472). A gate check of an app checks only the app's own crates, so the changed libs are
    # checked twice: alone (`check X`, their own features) and inside the cheapest affected
    # build that unifies the flip (`check B --also=X`: X's crates as B compiles them).
    def has_target(d, kinds=("lib", "rlib", "dylib")):
        return any(k in kinds for t in packages[d]["targets"] for k in t["kind"])

    def path_deps(d):
        # `cfg(any())` never holds: the commercial root's member list is never built or
        # feature-resolved (and that package has nothing to check)
        return [(Path(dep["path"]).resolve(), dep) for dep in packages[d]["dependencies"]
                if dep.get("path") and dep["kind"] is None and dep.get("target") != "cfg(any())"]
    # what a build compiles: normal dependencies that can hold (never `cfg(any())`)
    built = {d: {c for c, _ in path_deps(d)} for d in packages}
    reached = closure(touched, built)
    # what a build of the changed packages alone turns on already
    own = {(crate, f) for t in reached if t in packages for crate, dep in path_deps(t) for f in dep.get("features") or ()}
    flips = {}  # (crate, features) -> (packages that turn them on, the libs that compile differently)
    for directory in packages:
        for crate, dep in path_deps(directory):
            if crate in reached:
                features = frozenset(f for f in dep.get("features") or () if (crate, f) not in own)
                if features:
                    users = {t for t in touched if has_target(t) and crate in closure({t}, built)}
                    entry = flips.setdefault((crate, features), (set(), set()))
                    entry[0].add(directory)
                    entry[1].update(users)
    # A changed manifest that turns on more features of a dependency flips it for that
    # dependency's other direct users in every build of the changed package.
    for t in touched:
        manifest = t / "Cargo.toml"
        rel = str(manifest.relative_to(root))
        if rel.startswith("apps/commercial/"):
            repo, ref, rel = commercial, cbase, rel[len("apps/commercial/"):]
        else:
            repo, ref = root, base
        if ref == "-" or not any(p == str(manifest.relative_to(root)) for p in files):
            continue
        before = old_dep_features(repo, ref, rel, t)
        for crate, dep in path_deps(t):
            features = frozenset(dep.get("features") or ()) - before.get(crate, frozenset())
            if features and crate in packages:
                users = {d for d in packages if d != t and has_target(d)
                         and any(c == crate for c, _ in path_deps(d))}
                if users:
                    entry = flips.setdefault((crate, features), (set(), set()))
                    entry[0].add(t)
                    entry[1].update(users)
    holds = {d: closure({d}, built) for d in affected}
    also = {}  # build package name -> lib package names checked inside it
    alone = set()
    for (crate, features), (enablers, users) in sorted(flips.items(), key=lambda kv: (str(kv[0][0]), sorted(kv[0][1]))):
        for user in sorted(users):
            builds = [d for d in affected if holds[d] & enablers and user in holds[d] and d != user]
            if not builds:
                continue
            # a gate package already checked is the cheapest place; else the smallest build
            pick = packages[min(builds, key=lambda d: (packages[d]["name"] not in selected, len(holds[d]), packages[d]["name"]))]["name"]
            print(f"flip {packages[crate]['name']} {' '.join(sorted(features))}: {packages[user]['name']} in {pick}", file=sys.stderr)
            also.setdefault(pick, set()).add(packages[user]["name"])
            if user in touched:
                alone.add(packages[user]["name"])

    # Every changed package is checked itself too: an app's check covers only its own
    # crates, so a changed lib outside the gate packages would go unchecked
    # (makepad-sdf-engine's E0505 reached work, M47).
    alone.update(packages[d]["name"] for d in touched
                 if has_target(d, ("lib", "rlib", "dylib", "proc-macro", "bin")))

    def where(name):
        # Launch from the actual package directory; works for nested workspaces.
        return str(by_name[name].relative_to(root))

    def check_line(name):
        extra = f" --also={','.join(sorted(also[name]))}" if name in also else ""
        return f"check {name} {where(name)}{extra}"

    flipped = sorted(f"{b}:{','.join(sorted(x))}" for b, x in also.items())
    print(f"why files {len(files)}; changed: {' '.join(sorted(packages[d]['name'] for d in touched)) or '-'}; affected gate packages: {' '.join(sorted(selected)) or '-'}; feature flips: {' '.join(flipped) or '-'}")
    print("jit " + ("yes" if workspace or touched & jit_code else "no"))
    print("devhub " + ("yes" if workspace or touched & devhub_code else "no"))
    for name in GATE_PACKAGES:
        if name in selected:
            print(check_line(name))
    for name in sorted(set(also) - selected):
        print(check_line(name))
    for name in sorted(alone - selected - set(also)):
        print(f"check {name} {where(name)}")
    for name in SMOKE:
        if name in PRESENT_ARGS and name in selected:
            print(" ".join(["present", name, where(name), *PRESENT_ARGS[name]]))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
