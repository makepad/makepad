#!/usr/bin/env python3
"""Select the quick gate from both pinned diffs; run on a leased node only.

A gate checks and presents what the diff reaches (user 2026-10-30, landing design):
a change inside one app gates that app. The SMOKE apps all go when the diff reaches
the JIT itself (makepad-run's dependencies) or a workspace-wide input; the other
GATE_PACKAGES are checked when the diff reaches them, along with every changed
package with a lib or bin target. A diff that reaches the JIT checks JIT_CHECKS too, whatever
uses the changed crates. A diff that reaches no gate package gates
Calculator, the one canary; a diff only in devhub's own crate gates
its build alone. The other repo's crates that use a changed crate get a `cargo` line each, and so do
this repo's users that nothing else here type-checks (no gate package, unchanged, no test target): the
gate checks them all in one cargo check. Prints `jit yes|no`: whether the diff touches what the JIT binary
is built from; `devhub yes|no`: whether it touches devhub's own binary. A `check` line may end in
`--also=<libs>`: libs checked as that build compiles them (feature flips, below).
GATE_RETRY (units joined by `;`, from the hub's ledger) names what failed earlier and has not passed
since: selected whatever the diff reaches, as a diff that never reaches it says nothing about it. A
`gone <unit>` line: its package left the tree. A `suite <package>` line: a JIT suite failing since.
A `tests <package> <dir>` line: a package whose test targets the diff reaches, the changed packages
and every package that uses one (a dev-dependency too), in both repos: the gate type-checks them in
one `cargo check --tests` ([`tested`]). It ends in `--features=<package>/<feature>,..` when its users
turn on features of it ([`turned_on`]): it is checked with them whatever else that check holds. `--tests` prints only those lines, in any cargo workspace
(land_gate.sh tests). `--runs` prints the tests bucket's: `tests` lines of the packages whose tests it runs under
the JIT after a landing (land_gate.sh testrun, [`run_units`]): the changed packages and the gate packages the
diff reaches, as the fast bucket picks its checks; GATE_FULL=1 (the full bucket, daily) runs every package's,
whatever the diff. A package's test variants ([`variants`]) run beside it, a `tests` line each.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

# the smoke set, in run order: gated when the diff reaches the JIT or a workspace-wide input
SMOKE = ("makepad-stage", "makepad-scope", "makepad-sandbox", "makepad-fab", "makepad-app-synth", "makepad-app-calculator")
# checked too when the diff reaches them. The phone host (makepad-run-android) builds crates no
# smoke app does (its boot, the Android HAL paths): a change in any of them checks it on that
# landing, not on the next one that happens to check it (11-03)
GATE_PACKAGES = SMOKE + ("makepad-example-counter", "makepad-platform", "makepad-draw", "makepad-widgets", "makepad-run-android")
# checked too when the diff reaches the JIT itself: a JIT change reaches every package the JIT checks or runs,
# not its Rust dependents alone, and a smoke app's check covers only the app's own crates. A resolver change in
# relax-compiler broke makepad-widgets-core's check, and the bucket ran that check two landings later (11-05).
# A fixed set that stands for the rest (the smoke set's commercial apps for a commercial app)
JIT_CHECKS = ("makepad-widgets-core", "makepad-platform", "makepad-draw", "makepad-example-counter", "makepad-story-web-export")
CALC = "makepad-app-calculator"
# GPU guards, (package, target, test filter): render tests of a user-visible effect that broke more than once with
# every test green. The target is `lib` (the package's library tests) or `test:<name>` (one of its test targets). The
# hot bucket runs one on its node's real GPU under the JIT after every landing that reaches its package
# (land_gate.sh step_guards): the map's zoom into text, its phosphor lens (11-06); Fab's far wall face in the
# headset, its layers even (11-11)
GUARDS = (
    ("makepad-scope", "lib", "gpu_guard"),
    ("makepad-fab", "test:xr_far_face", "a_far_wall_face_shows_no_darker_layers_in_the_headset"),
)
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


def tested(packages, members, touched, retry):
    """The packages whose test targets the diff reaches: the changed ones, those that use them
    (normal and build edges, as `affected` follows), and those whose tests use one of these (a
    dev-dependency reaches a package's tests, not its users), with a test target each, of the
    root workspace (`members`: cargo tests no package outside it). No check
    compiles a `#[cfg(test)]` body: two landings that merged cleanly broke makepad-fab's tests, one
    adding a test that called `build_preview` with an argument the other took away, and every
    bucket said ok (11-05). Sorted (name, dir) pairs."""
    reverse, dev = {}, {}
    for directory, pkg in packages.items():
        for dep in pkg["dependencies"]:
            if dep.get("path"):
                (dev if dep["kind"] == "dev" else reverse).setdefault(Path(dep["path"]).resolve(), set()).add(directory)
    reached = closure(touched, reverse)
    reached |= {user for d in reached for user in dev.get(d, ())}
    has_tests = {d for d in members if any(t.get("test", True) for t in packages[d]["targets"])}
    names = {packages[d]["name"] for d in reached & has_tests}
    by_name = {p["name"]: d for d, p in packages.items()}
    # a test-target check (`tests <pkg>`) or a run of the tests (`test <pkg>`, the tests bucket) red since
    names |= {words[1] for words in retry if words[0] in ("tests", "test") and by_name.get(words[1]) in has_tests}
    return sorted((name, by_name[name]) for name in names)


def turned_on(packages, directory):
    """The features of the package at `directory` that its users' path dependencies turn on (any
    edge kind that can hold). Features unify per build, so a target that needs one (a bin's
    `required-features`, a `#[cfg(feature)]` body) compiles only in a check that also holds such
    a user. makepad-ai-hub's multiview_check bin (required-features multiview-native, which the AI
    hub app turns on) failed the fast bucket's one test check beside the app. Checked again alone,
    as a retry, the bin was skipped and the break read as fixed (11-08). Sorted."""
    own = set(packages[directory].get("features") or {})
    on = set()
    for d, pkg in packages.items():
        if d == directory:
            continue
        for dep in pkg["dependencies"]:
            if dep.get("path") and dep.get("target") != "cfg(any())" and Path(dep["path"]).resolve() == directory:
                on.update(f for f in dep.get("features") or () if f in own)
    return sorted(on)


def variants(pkg):
    """The features a package's tests also run with, each set a unit of its own (its Cargo.toml's
    `[package.metadata.makepad] test-variants`, e.g. `["web"]`): a feature that swaps what the package
    builds, which no user turns on and no other build may hold, such as a package's portable `web`
    build: without a unit of its own nothing built those tests, and they stopped compiling."""
    return ((pkg.get("metadata") or {}).get("makepad") or {}).get("test-variants") or []


def tests_line(packages, name, directory, root):
    """A `tests` line: the package, its directory, and the features its users turn on ([`turned_on`])."""
    on = turned_on(packages, directory)
    extra = f" --features={','.join(f'{name}/{f}' for f in on)}" if on else ""
    return f"tests {name} {directory.relative_to(root)}{extra}"


def nested_bases():
    """The other nested repositories a bucket's tip pins, each with its base commit
    (GATE_NESTED_BASES=<name>=<base sha|->,..)."""
    return [tuple(side.split("=", 1)) for side in os.environ.get("GATE_NESTED_BASES", "").split(",") if side]


def changed(root, base, cbase):
    """The paths the diff changed, both repos (commercial ones under apps/commercial/)."""
    # --no-renames includes both the removed and added path; NUL keeps spaces safe.
    files = [p for p in git(root, "diff", "--no-renames", "--name-only", "-z", base, "HEAD").split("\0") if p]
    commercial = root / "apps/commercial"
    if cbase != "-":
        if not (commercial / ".git").exists():
            raise RuntimeError("commercial base supplied but checkout is missing")
        files += ["apps/commercial/" + p for p in git(commercial, "diff", "--no-renames", "--name-only", "-z", cbase, "HEAD").split("\0") if p]
    # each other nested repository's diff from its base under apps/<name>/; `-`: the bucket's base
    # had none of it, so every file of it is new
    for name, nbase in nested_bases():
        nested = root / "apps" / name
        if not (nested / ".git").exists():
            raise RuntimeError(f"{name} base supplied but its checkout is missing")
        listed = git(nested, "ls-files", "-z") if nbase == "-" else git(nested, "diff", "--no-renames", "--name-only", "-z", nbase, "HEAD")
        files += [f"apps/{name}/" + p for p in listed.split("\0") if p]
    return files


def owners(root, packages, files):
    """The packages that hold the compiled paths of `files`: the gate's own scripts and docs
    compile into nothing."""
    def owner(path):
        path = root / path
        return next((parent for parent in (path.parent, *path.parents)
                     if parent in packages), None)
    compiled = [p for p in files if not p.startswith("tools/run/fixture/") and not p.endswith(".md")]
    return compiled, {owner(path) for path in compiled} - {None}


def retried():
    """GATE_RETRY's units as word lists (a word that is no unit, `steps`, is the gate's own)."""
    return [words for words in (unit.split() for unit in os.environ.get("GATE_RETRY", "").split(";")) if len(words) >= 2]


def local(root, meta, packages):
    """Adds the tree's own packages of one `cargo metadata` to `packages` (by directory); their
    directories that are members of its workspace."""
    members = set(meta["workspace_members"])
    out = set()
    for pkg in meta["packages"]:
        directory = Path(pkg["manifest_path"]).resolve().parent
        if pkg["source"] is None and root in directory.parents:
            packages[directory] = pkg
            if pkg["id"] in members:
                out.add(directory)
    return out


def built_from(packages, name):
    """The directories a package's binary is built from: it and the path crates it uses (normal and
    build edges), or none when the tree has no such package."""
    forward = {directory: {Path(d["path"]).resolve() for d in pkg["dependencies"] if d.get("path") and d["kind"] != "dev"}
               for directory, pkg in packages.items()}
    top = next((d for d, p in packages.items() if p["name"] == name), None)
    return closure({top}, forward) if top else set()


def workspace_wide(compiled):
    """Whether the diff changed a resolver/patch/profile input of every member."""
    return any(p in ("Cargo.toml", "Cargo.lock", ".cargo/config", ".cargo/config.toml") for p in compiled)


def run_units(packages, members, touched, workspace, jit, retry):
    """The packages whose tests the tests bucket runs after a landing: the changed ones, and the gate
    packages the diff reaches (GATE_PACKAGES, through normal and build edges; every one on a workspace-wide
    input), the smoke set and JIT_CHECKS too when it reaches the JIT or a workspace-wide input: the fast
    bucket's selection, of the root workspace's packages with a test target; and a unit red since. Not
    every user of a changed crate: a change to the JIT or makepad-platform reached all 547 units, and the
    bucket held most of the pool's boxes for 78-106 min a run, a new run queued as one ended
    (11-08). The full bucket runs every package's daily; the fast bucket type-checks every reached test
    target ([`tested`]). Sorted (name, dir) pairs."""
    reverse = {}
    for directory, pkg in packages.items():
        for dep in pkg["dependencies"]:
            if dep.get("path") and dep["kind"] != "dev":
                reverse.setdefault(Path(dep["path"]).resolve(), set()).add(directory)
    by_name = {p["name"]: d for d, p in packages.items()}
    reached = set(packages) if workspace else closure(touched, reverse)
    gate = {by_name[name] for name in GATE_PACKAGES if by_name.get(name) in reached}
    if workspace or jit:
        gate |= {by_name[name] for name in SMOKE + JIT_CHECKS if name in by_name}
    has_tests = {d for d in members if any(t.get("test", True) for t in packages[d]["targets"])}
    names = {packages[d]["name"] for d in (touched | gate) & has_tests}
    names |= {words[1] for words in retry if words[0] in ("tests", "test") and by_name.get(words[1]) in has_tests}
    return sorted((name, by_name[name]) for name in names)


def tests_only(root, base, cbase, runs):
    packages = {}
    members = local(root, metadata(root / "Cargo.toml"), packages)
    compiled, touched = owners(root, packages, changed(root, base, cbase))
    workspace, jit = workspace_wide(compiled), bool(touched & built_from(packages, JIT_PACKAGE))
    if os.environ.get("GATE_FULL") == "1":
        units = tested(packages, members, set(members), retried())
    elif runs:
        units = run_units(packages, members, touched, workspace, jit, retried())
    else:
        # the test targets the diff reaches; a JIT or workspace-wide change reaches every package's
        units = tested(packages, members, set(members) if workspace or jit else touched, retried())
    for name, directory in units:
        # (the tests bucket's runs take the package's own features, as `makepad test` does)
        print(f"tests {name} {directory.relative_to(root)}" if runs else tests_line(packages, name, directory, root))
        for features in variants(packages[directory]) if runs else ():
            print(f"tests {name} {directory.relative_to(root)} --features {features}")


def main():
    root = Path.cwd().resolve()
    if sys.argv[1] in ("--tests", "--runs"):
        return tests_only(root, *sys.argv[2:4], sys.argv[1] == "--runs")
    base, cbase = sys.argv[1:3]
    files = changed(root, base, cbase)
    commercial = root / "apps/commercial"

    # Key by manifest directory, not Cargo package ID: separate workspaces can
    # assign different IDs to the same path dependency. Keep excluded path libs
    # too, so changes there propagate through the entire dependency chain.
    packages = {}
    members = local(root, metadata(root / "Cargo.toml"), packages)
    # Older commercial tips keep Sandbox outside the enclosing workspace.
    sandbox = commercial / "sandbox/Cargo.toml"
    if sandbox.exists() and not any(p["name"] == "makepad-sandbox" for p in packages.values()):
        local(root, metadata(sandbox), packages)

    reverse = {}
    for directory, pkg in packages.items():
        # Use declared path dependencies, including optional and target-specific
        # edges; cargo's host/default resolve graph can hide affected consumers.
        for dep in pkg["dependencies"]:
            if dep.get("path") and dep["kind"] != "dev":
                reverse.setdefault(Path(dep["path"]).resolve(), set()).add(directory)

    compiled, touched = owners(root, packages, files)
    affected = closure(touched, reverse)
    by_name = {p["name"]: d for d, p in packages.items()}
    missing = [name for name in SMOKE + JIT_CHECKS + tuple(name for name, _, _ in GUARDS) if name not in by_name]
    if missing:
        raise RuntimeError("smoke or JIT check packages missing from metadata: " + " ".join(missing))

    # What the JIT binary and devhub's are built from.
    jit_code = built_from(packages, JIT_PACKAGE)
    devhub_dir = by_name.get(DEVHUB_PACKAGE)
    devhub_code = built_from(packages, DEVHUB_PACKAGE)
    # Workspace-wide resolver/patch/profile inputs apply to every member.
    workspace = workspace_wide(compiled)
    retry = retried()
    gone = [words for words in retry if words[1] not in by_name or (words[0] == "present" and words[1] not in PRESENT_ARGS)]
    live = [words for words in retry if words not in gone]
    retry_presents = {words[1] for words in live if words[0] == "present"}
    retry_devhub = any(words[0] == "devhub" for words in live)
    retry_why = "; retry: " + ", ".join(" ".join(words) for words in retry) if retry else ""

    def print_retried():
        for words in gone:
            print("gone " + " ".join(words))
        for words in live:
            if words[0] == "suite":
                print("suite " + words[1])
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
        selected |= retry_presents
        print(f"why files {len(files)}; changed: {' '.join(sorted(packages[d]['name'] for d in touched)) or '-'}; affected gate packages: {' '.join(sorted(selected)) or '-'}; feature flips: -{retry_why}")
        print("jit " + ("yes" if workspace or touched & jit_code else "no"))
        print("devhub " + ("yes" if workspace or touched & devhub_code else "no"))
        print_retried()
        for name in SMOKE:
            if name in selected:
                print(" ".join(["present", name, str(by_name[name].relative_to(root)), *PRESENT_ARGS[name]]))
        # a guard runs under the JIT: a JIT change reaches it as much as one of its package's crates; a guard
        # red since runs whatever the diff reaches, until it passes
        retry_guards = {words[1] for words in live if words[0] == "guard"}
        for name, target, test in GUARDS:
            if workspace or by_name[name] in affected or touched & jit_code or name in retry_guards:
                print(f"guard {name} {by_name[name].relative_to(root)} {target} {test}")
        return
    if workspace:
        affected.update(packages)
    selected = {name for name in GATE_PACKAGES if by_name.get(name) in affected}
    if workspace or touched & jit_code:
        selected |= set(SMOKE) | set(JIT_CHECKS)
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
        sides = [("commercial", cbase)] + nested_bases()
        side = next(((root / "apps" / name, nbase, rel[len(f"apps/{name}/"):]) for name, nbase in sides if rel.startswith(f"apps/{name}/")), None)
        repo, ref, rel = side or (root, base, rel)
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
    # (a lib's E0505 reached work, M47).
    alone.update(packages[d]["name"] for d in touched
                 if has_target(d, ("lib", "rlib", "dylib", "proc-macro", "bin")))

    # The other repo's crates that use a changed crate, through an edge that can hold (the
    # commercial member list's `cfg(any())` never does): an OSS crate's removed fn broke a
    # commercial crate no gate package reached, and every fast bucket said ok (11-03). One cargo
    # check takes them all (98 users of makepad-platform: 26 s on a pool box after an edit of
    # it); a JIT check each would be a hundred. A workspace-wide input alone adds none: the smoke
    # set stands for it.
    users = {}
    for d in packages:
        for crate, _ in path_deps(d):
            users.setdefault(crate, set()).add(d)

    # what failed earlier, as it failed: a flip's lib that left the tree is no longer in that build
    for words in live:
        if words[0] == "check":
            libs = {lib for word in words[2:] if word.startswith("--also=") for lib in word[len("--also="):].split(",") if lib in by_name}
            if libs:
                also.setdefault(words[1], set()).update(libs)
            else:
                alone.add(words[1])

    def private(d):
        return d == commercial or commercial in d.parents
    checked = selected | set(also) | alone
    tests = tested(packages, members, touched, live)
    # and a user in the same repo that no line here type-checks: no gate package, not changed, no test target
    # (the test-target check builds the others' libs): every user of a changed crate is checked on its landing,
    # not when a later diff happens to reach it
    covered = checked | {name for name, _ in tests}
    other = {packages[d]["name"] for t in touched for d in closure({t}, users) - {t}
             if has_target(d, ("lib", "rlib", "dylib", "proc-macro", "bin"))
             and (private(d) != private(t) or (d in members and packages[d]["name"] not in covered))} - checked
    other |= {words[1] for words in live if words[0] == "cargo"}

    def where(name):
        # Launch from the actual package directory; works for nested workspaces.
        return str(by_name[name].relative_to(root))

    def check_line(name):
        extra = f" --also={','.join(sorted(also[name]))}" if name in also else ""
        return f"check {name} {where(name)}{extra}"

    flipped = sorted(f"{b}:{','.join(sorted(x))}" for b, x in also.items())
    print(f"why files {len(files)}; changed: {' '.join(sorted(packages[d]['name'] for d in touched)) or '-'}; affected gate packages: {' '.join(sorted(selected)) or '-'}; feature flips: {' '.join(flipped) or '-'}{retry_why}")
    print("jit " + ("yes" if workspace or touched & jit_code else "no"))
    print("devhub " + ("yes" if workspace or touched & devhub_code or retry_devhub else "no"))
    print_retried()
    for name in GATE_PACKAGES + tuple(n for n in JIT_CHECKS if n not in GATE_PACKAGES):
        if name in selected:
            print(check_line(name))
    for name in sorted(set(also) - selected):
        print(check_line(name))
    for name in sorted(alone - selected - set(also)):
        print(f"check {name} {where(name)}")
    for name in sorted(other):
        print(f"cargo {name} {where(name)}")
    for name, directory in tests:
        print(tests_line(packages, name, directory, root))
    for name in SMOKE:
        if name in PRESENT_ARGS and (name in selected or name in retry_presents):
            print(" ".join(["present", name, where(name), *PRESENT_ARGS[name]]))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
