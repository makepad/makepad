#!/usr/bin/env python3
"""Nothing breaks a running app: real landings replayed as live edits under this tree's JIT.

usage: live_replay.py --app <package> [--count N] [--only <sha,..>] [--landings <path>] [--history <makepad>[,apps/<name>=<commit>..]]
       live_replay.py --host --app <package> [--wm]
       live_replay.py --chain --app <package> --only <sha,..>   (one app takes them in turn, no restart)
  (run from the workspace root; results also go to target/live_replay/results-<app>.jsonl)

The JIT under test is always this tree's. The app's tree is a second tree (in the temp dir, one index
per repository: makepad and each one cloned into apps/<name>, moved with `git read-tree -u` like a sync
moves a node) at the landing's parent, except
the crates the JIT binary provides (makepad-run's #[relax_api] crates and their path deps) and the
JIT's own: those stay at this tree, as the loader requires them to match its build.

A landing moves all participating repositories together if any half changes a crate the app builds.
--landings reads devhub's ordered JSONL land records. Without it, matching devhub merge subjects
within five seconds are paired from Git. Direct commits stay separate. Replay runs newest first.
Far enough back the tree no longer builds against
this tree's JIT and provided crates: after DRIFTED such landings in a row the run ends there:
  1. the app runs at the parent, first frame, settled, answers input; its layout at three window
     sizes, then again after `session relaunch` with no edit: the nodes that move are the app's
     state (what a restart keeps or loses), skipped by the layout checks below;
  2. the tree moves to the landing, as a sync does; the JIT's verdict is awaited;
  3. the app is alive, draws and answers input, no panic, ICE, parked thread or error view;
  4. `session relaunch` (the restart button), on the warm cache of the parent: the same again;
  5. an applied edit lays out as the restart does (stale_layout), and the restart as a new
     process of the same code (stale_restart: something of the previous run outlived it).
The verdict is the user's rule, "restart yes, update yes, crash no":
  pass: applied, or a restart that comes up working (the edit's rung is only recorded);
  fail: a crash, ICE, panic or parked thread, a dead UI, or an error view over the app;
  silent: the watcher took the edit up and gave no verdict: a bug, though the app runs on;
  skip: the tree itself does not build: rustc rejects it too (old crates against this tree's provided
        crates), so it says nothing about the JIT. rustc accepting what the JIT rejects is a fail;
  unreached: the app loads nothing the landing changed, no verdict to judge: not a pass.
--history takes the landings from those commits' first parents instead of this tree's (a history cut
down to one commit has none to replay): makepad's, then any nested repository's as apps/<name>=<commit>
(the others at their HEAD). A control edit at the start of a run must get a verdict and the health check must see an error view
over the app, or the run stops: its checks could not fail.

--host: the path replay cannot reach, a landing that changes what makepad itself is built from. The
app runs from this tree, as a dock app runs from its node's checkout (the edits go where the job's end
resets them). Each case edits a provided crate or makepad run's own sources: the app offers the restart,
`session relaunch` is the click, makepad rebuilds in the background, and the app must come back working
in the same process id with its state handed over. --wm: the app is a hosted window of a WM.

Ends with one number per app: `live_replay: <app> pass rate P% (pass/judged, ...)`; exit 3 on a miss, 1
when the run could judge nothing.
"""
from __future__ import annotations

import bisect
import json, os, re, shutil, subprocess, sys, tempfile, time, zlib
from typing import NamedTuple

ROOT = os.getcwd()
WORK = os.path.join(ROOT, "target", "live_replay")
# outside the workspace: inside it cargo would merge both .cargo/config.toml files, the doubled
# flags a different fingerprint from the workspace's own, and every `cargo run` a rebuild
TREE = os.path.join(tempfile.gettempdir(), "live_replay_tree")
# beside the tree it describes: a move writes only what differs from the index, so an index that
# outlived its tree (the tree in a lease's temp dir, the index in target/) left files unwritten
INDEX = os.path.join(tempfile.gettempdir(), "live_replay_index")


def nested_repos():
    """The repositories cloned into apps/<name> of this checkout (makepad/apps, the commercial
    products, Source Library): the second tree holds each at its own commit, as a box's tree does.
    It held only apps/commercial: once Fab moved into apps/apps, makepad-fab was no member of the
    second tree's workspace and its control app exited (1) before its first frame (11-23)."""
    apps = os.path.join(ROOT, "apps")
    # (named as devhub names them: lowercase letters, digits and dashes)
    return sorted("apps/" + n for n in os.listdir(apps) if re.fullmatch(r"[a-z0-9-]+", n) and os.path.exists(os.path.join(apps, n, ".git"))) if os.path.isdir(apps) else []


NESTED = nested_repos()
# git's empty tree: a nested repository with no commit at a landing's time is absent from its tree
EMPTY = "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
DEVHUB_LAND = re.compile(r"^Merge branch '([^']+)' into (\S+) \(devhub land\)$")
LAND_WINDOW = 5  # seconds between the first and last merge of a git-only pair
# the JIT binary's providers (tools/run/Cargo.toml's host-shims dependencies)
PROVIDERS = ["makepad-live-id", "makepad-script", "makepad-script-std", "makepad-script-compute", "makepad-shader", "makepad-tween-script", "makepad-script-audio-aot", "makepad-hal", "makepad-script-census", "relax-api"]
JIT = ["cargo", "run", "--profile", "host", "-q", "--manifest-path", os.path.join(ROOT, "Cargo.toml"), "-p", "makepad-run", "--bin", "makepad", "--"]
# The inspect client is a copy of this tree's binary: only a client of the app's socket, and in --host
# the tree it would be rebuilt from changes under it (a cargo run per ask would race the JIT's rebuild).
CLIENT = os.path.join(WORK, "makepad-client")
REBUILD = 1500  # a rebuild of makepad on a busy box, from the click to the restart
COLD = 1500  # a cold first start of a big app on a busy box
BOOT = 600  # a start after a tree move
VERDICT = int(os.environ.get("LIVE_REPLAY_VERDICT", "180"))  # save to the JIT's verdict on the edit
SETTLE = 120  # the JIT idle after a frame
LIVE_POLL = 2.0  # how often waits ask the JIT `live` (and `session` for host rebuilds)
DRIFTED = 5  # landings in a row whose tree rustc rejects here: history older than this JIT can run
RESIZE = "--no-resize" not in sys.argv  # an applied edit's layout is compared with a fresh start's
UNSEEN = 20  # no word from the watcher this long: the edit touched nothing the app loaded


def opt(name, default):
    args = sys.argv[1:]
    return args[args.index(name) + 1] if name in args else default


def git(*a, env=None, ok=True):
    r = subprocess.run(["git", *a], cwd=ROOT, env=env, capture_output=True, text=True)
    if ok and r.returncode != 0:
        raise SystemExit("git %s: %s" % (" ".join(a), r.stderr.strip()))
    return r.stdout.strip()


def ngit(repo, *a, env=None, ok=True):
    """git in the nested repository `repo` (apps/<name>)."""
    r = subprocess.run(["git", *a], cwd=os.path.join(ROOT, repo), env=env, capture_output=True, text=True)
    if ok and r.returncode != 0:
        raise SystemExit("git (%s) %s: %s" % (repo, " ".join(a), r.stderr.strip()))
    return r.stdout.strip()


def index_of(repo):
    """The second tree's index of the nested repository `repo`."""
    return INDEX + "." + os.path.basename(repo)


def under(d):
    """The nested repository the workspace dir `d` is in, or None (makepad's)."""
    return next((r for r in NESTED if d == r or d.startswith(r + "/")), None)


class Landing(NamedTuple):
    t: int
    order: int  # position in the land records, including same-second lands
    commits: tuple[tuple[str, str], ...]
    land_id: int | None = None
    pre: tuple[tuple[str, str], ...] = ()
    post: tuple[tuple[str, str], ...] = ()


def repo_rank(repo: str) -> tuple[int, str]:
    return (0, "") if repo == "makepad" else (1, repo)


def canonical_commits(mapping: dict[str, str]) -> tuple[tuple[str, str], ...]:
    return tuple(sorted(mapping.items(), key=lambda kv: repo_rank(kv[0])))


def primary_sha(landing: Landing) -> str:
    """The commit whose 10-hex prefix identifies this landing in CI output."""
    return landing.commits[0][1]


def landing_sort_key(landing: Landing) -> tuple:
    return (landing.t, 0 if landing.land_id is None else 1, landing.order, landing.commits)


def repo_git(repo: str, *args, env=None, ok=True) -> str:
    return git(*args, env=env, ok=ok) if repo == "makepad" else ngit(repo, *args, env=env, ok=ok)


def repo_parent(repo: str, sha: str, cache: dict[tuple[str, str], str] | None = None) -> str:
    """First parent of sha in repo, or the empty tree for a nested root commit."""
    key = (repo, sha)
    if cache is not None and key in cache:
        return cache[key]
    p = repo_git(repo, "rev-parse", "--verify", "-q", sha + "^1", ok=False)
    if not p:
        if repo == "makepad":
            raise SystemExit("live_replay: makepad commit %s has no first parent" % sha[:10])
        p = EMPTY
    if cache is not None:
        cache[key] = p
    return p


def repo_at_before(repo: str, t: int, upper: str) -> str:
    return repo_git(repo, "log", "--first-parent", "-1", "--format=%H", "--before=%d" % t, upper)


def is_ancestor(repo: str, anc: str, desc: str) -> bool:
    r = subprocess.run(["git", "merge-base", "--is-ancestor", anc, desc],
                       cwd=ROOT if repo == "makepad" else os.path.join(ROOT, repo), capture_output=True)
    return r.returncode == 0


class LandIndex:
    def __init__(self, records: list[Landing], trimmed: dict[str, int] | None = None):
        self.records = records
        self.trimmed = trimmed
        self.by_commit: dict[tuple[str, str], int] = {}
        self.by_repo: dict[str, list[tuple[int, str]]] = {}
        self.by_repo_idx: dict[str, list[int]] = {}
        self.by_repo_t: dict[str, list[int]] = {}
        for idx, rec in enumerate(records):
            for repo, sha in rec.commits:
                self.by_commit[(repo, sha)] = idx
                self.by_repo.setdefault(repo, []).append((idx, sha))
                self.by_repo_idx.setdefault(repo, []).append(idx)
                self.by_repo_t.setdefault(repo, []).append(rec.t)

    def repo_bounds(self, repo: str, idx: int) -> tuple[str | None, str | None]:
        """Previous and next landed SHAs in record order, strictly around idx."""
        indices = self.by_repo_idx.get(repo, [])
        rows = self.by_repo.get(repo, [])
        left = bisect.bisect_left(indices, idx)
        right = bisect.bisect_right(indices, idx)
        return (rows[left - 1][1] if left else None, rows[right][1] if right < len(rows) else None)

    def next_after_time(self, repo: str, t: int) -> str | None:
        """First landing of repo strictly after t."""
        i = bisect.bisect_right(self.by_repo_t.get(repo, []), t)
        rows = self.by_repo.get(repo, [])
        return rows[i][1] if i < len(rows) else None

    def present(self, idx, all_tips):
        """The record's exact states for the repositories installed in this checkout."""
        rec = self.records[idx]
        def included(pairs):
            return tuple((r, c) for r, c in pairs if r in all_tips)
        return rec._replace(order=idx, commits=included(rec.commits), pre=included(rec.pre), post=included(rec.post))


def load_landings(path: str) -> LandIndex:
    """Read compact JSONL land records in devhub's (changed_ms, id) order."""
    records = []
    trimmed = None
    try:
        with open(path, encoding="utf-8") as source:
            for lineno, line in enumerate(source, 1):
                if not line.strip():
                    continue
                try:
                    row = json.loads(line)
                    if not isinstance(row, dict) or type(row.get("id")) is not int or type(row.get("t")) is not int:
                        raise ValueError("expected a land record with integer id and t")
                    if not row.get("commits"):
                        raise ValueError("expected non-empty commits")
                    if "trimmed" in row:
                        if records:
                            raise ValueError("trimmed is only valid on the first record")
                        trimmed = row["trimmed"]
                        if (not isinstance(trimmed, dict) or trimmed.keys() != {"window", "older", "newer", "total"}
                                or any(type(n) is not int or n < 0 for n in trimmed.values())):
                            raise ValueError("expected trimmed to contain non-negative integers for window, older, newer and total")
                    maps = []
                    for field in ("commits", "pre", "post"):
                        entries = row.get(field, {})
                        if not isinstance(entries, dict) or any(
                            not isinstance(c, str) or not re.fullmatch(r"[0-9a-fA-F]{40}", c) for c in entries.values()
                        ):
                            raise ValueError("expected %s to map repository names to 40-hex SHAs" % field)
                        maps.append(canonical_commits({r: c.lower() for r, c in entries.items()}))
                    commits, pre, post = maps
                    records.append(Landing(row["t"], len(records), commits, row["id"], pre, post))
                except ValueError as e:
                    raise SystemExit("live_replay: %s:%d: %s" % (path, lineno, e))
    except (OSError, UnicodeError) as e:
        raise SystemExit("live_replay: cannot read %s: %s" % (path, e))
    return LandIndex(records, trimmed)


def loaded_line(index: LandIndex, path: str) -> str:
    line = "live_replay: loaded %d land records from %s" % (len(index.records), path)
    trim = index.trimmed
    if trim is not None:
        line += (f" (the master's {trim['window']}-row window left out {trim['older']} older and "
                 f"{trim['newer']} newer of {trim['total']}; the oldest kept is #{index.records[0].land_id})")
    return line


def metadata():
    return json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--offline"], cwd=ROOT, capture_output=True, text=True, check=True).stdout)


def closure(meta, names):
    """The dirs (in the workspace) of these packages and their path dependencies, outermost only."""
    by_id = {p["id"]: p for p in meta["packages"]}
    deps = {n["id"]: n["dependencies"] for n in meta["resolve"]["nodes"]}
    seen, todo = set(), [p["id"] for p in meta["packages"] if p["name"] in names]
    while todo:
        i = todo.pop()
        if i not in seen and by_id[i]["source"] is None:
            seen.add(i)
            todo.extend(deps.get(i, []))
    return {os.path.relpath(os.path.dirname(by_id[i]["manifest_path"]), ROOT) for i in seen}


def outermost(dirs):
    return sorted(d for d in dirs if not any(d != o and d.startswith(o + "/") for o in dirs))


def kept_dirs(meta):
    """The tree's dirs that stay at this tree: the providers' path dependency closure, the JIT's."""
    return outermost(closure(meta, PROVIDERS) | {"libs/relax", "tools/run", ".cargo"})


def git_commits(repo, *args):
    log = repo_git(repo, "log", "--first-parent", "--format=%ct\t%H\t%s", *args)
    for line in log.splitlines():
        parts = line.split("\t", 2)
        yield int(parts[0]), repo, parts[1], parts[2] if len(parts) > 2 else ""


def same_branch_groups(rows, window):
    """Join closest commits first, keeping one per repository and the whole span within window."""
    rows = sorted(rows)
    groups = {row: {row} for row in rows}
    pairs = []
    for i, left in enumerate(rows):
        for j in range(i + 1, len(rows)):
            right = rows[j]
            dt = right[0] - left[0]
            if dt > window:
                break
            if left[1] != right[1]:
                pairs.append((dt, left, right))
    for _, left, right in sorted(pairs):
        if groups[left] is groups[right]:
            continue
        members = groups[left] | groups[right]
        if max(members)[0] - min(members)[0] > window or len({r for _, r, _ in members}) != len(members):
            continue
        for row in members:
            groups[row] = members
    for row, members in groups.items():
        if row == min(members):
            yield Landing(row[0], 0, canonical_commits({r: c for _, r, c in members}))


def group_git_landings(
    candidates: list[tuple[int, str, str, str]], all_tips: dict[str, str],
    claimed: set[tuple[str, str]], window: int = LAND_WINDOW,
) -> list[Landing]:
    """Find other halves even outside the app's closure, without claiming an indexed half."""
    wanted = {(r, c): (t, s) for t, r, c, s in candidates if (r, c) not in claimed}
    result = []
    branches = {}
    times = []
    for (repo, sha), (t, subject) in wanted.items():
        match = DEVHUB_LAND.fullmatch(subject)
        if match:
            branches.setdefault(match[1], {})[(repo, sha)] = t
            times.append(t)
        else:
            result.append(Landing(t, 0, ((repo, sha),)))
    if times:
        bounds = ("--since=%d" % (min(times) - window), "--until=%d" % (max(times) + window))
        for repo, tip in all_tips.items():
            for t, _, sha, subject in git_commits(repo, *bounds, tip):
                match = DEVHUB_LAND.fullmatch(subject)
                if match and match[1] in branches and (repo, sha) not in claimed:
                    branches[match[1]][(repo, sha)] = t
        for commits in branches.values():
            rows = [(t, r, c) for (r, c), t in commits.items()]
            result.extend(group for group in same_branch_groups(rows, window)
                          if any(key in wanted for key in group.commits))
    return result


def landings(
    meta: dict, package: str, kept: list[str], tip: str, tips: dict[str, str], count: int,
    land_index: LandIndex | None = None,
) -> list[Landing]:
    """Up to count atomic landings changing a crate the app builds, oldest first."""
    if count <= 0:
        return []
    dirs = outermost(closure(meta, [package]))
    candidates = []
    excl = [":(exclude)%s" % d for d in kept]
    ours = [d for d in dirs if under(d) is None and d != "."]
    if ours:
        candidates.extend(git_commits("makepad", "-%d" % count, tip, "--", *ours, *excl))
    for repo in NESTED:
        theirs = [os.path.relpath(d, repo) for d in dirs if under(d) == repo]
        if theirs:
            candidates.extend(git_commits(repo, "-%d" % count, tips[repo], "--", *theirs))

    all_tips = {"makepad": tip, **tips}
    indexed = land_index.by_commit if land_index is not None else {}
    matched = set()
    unmatched = []
    for candidate in candidates:
        _, repo, sha, _ = candidate
        if (repo, sha) in indexed:
            matched.add(indexed[(repo, sha)])
        else:
            unmatched.append(candidate)
    seen = {(r, c) for _, r, c, _ in candidates}
    result = []
    for idx in matched:
        rec = land_index.present(idx, all_tips)
        if all((r, c) in seen or is_ancestor(r, c, all_tips[r]) for r, c in rec.commits):
            result.append(rec)
    if unmatched:
        result.extend(group_git_landings(unmatched, all_tips, set(indexed)))
    return sorted(result, key=landing_sort_key)[-count:]


def resolve_only(only: list[str], all_tips: dict[str, str], land_index: LandIndex | None = None) -> list[Landing]:
    """Resolve explicit revisions and emit each complete landing once, in request order."""
    indexed = land_index.by_commit if land_index is not None else {}
    claimed = set(indexed)
    emitted = set()
    result = []
    for rev in only:
        for repo in ["makepad", *NESTED]:
            sha = repo_git(repo, "rev-parse", "--verify", "-q", rev + "^{commit}", ok=False)
            if sha:
                break
        else:
            raise SystemExit("live_replay: --only %s: no commit of makepad or %s" % (rev, " ".join(NESTED) or "a nested repository"))
        if (repo, sha) in emitted:
            continue
        if (repo, sha) in indexed:
            landing = land_index.present(indexed[(repo, sha)], all_tips)
            for r, c in landing.commits:
                if not repo_git(r, "rev-parse", "--verify", "-q", c + "^{commit}", ok=False):
                    raise SystemExit("live_replay: landing #%s: missing commit %s in %s" % (landing.land_id, c, r))
        else:
            candidates = list(git_commits(repo, "-1", sha))
            landing, = group_git_landings(candidates, all_tips, claimed)
        result.append(landing)
        emitted.update(landing.commits)
        claimed.update(landing.commits)
    return result


def resolve_states(
    landing: Landing, all_tips: dict[str, str], tree_at, land_index: LandIndex | None = None,
    parent_cache: dict[tuple[str, str], str] | None = None,
) -> tuple[tuple[str, tuple[tuple[str, str], ...]], tuple[str, tuple[tuple[str, str], ...]]]:
    """Resolve both sides of a land, including all moved repos and the bounds on unchanged repos."""
    moved, pre, post = dict(landing.commits), dict(landing.pre), dict(landing.post)
    before, after = {}, {}
    for repo in ["makepad", *NESTED]:
        if repo in moved:
            after[repo] = moved[repo]
            before[repo] = pre[repo] if repo in pre else repo_parent(repo, moved[repo], parent_cache)
            continue
        if repo in pre or repo in post:
            commit = pre[repo] if repo in pre else post[repo]
        else:
            prev_sha = next_sha = None
            if land_index is not None:
                if landing.land_id is not None:
                    prev_sha, next_sha = land_index.repo_bounds(repo, landing.order)
                else:
                    next_sha = land_index.next_after_time(repo, landing.t)
            upper = all_tips[repo]
            if next_sha and is_ancestor(repo, next_sha, upper):
                upper = repo_parent(repo, next_sha, parent_cache)
            if upper == prev_sha or upper == EMPTY:
                commit = upper
            else:
                commit = repo_at_before(repo, landing.t, upper) or prev_sha or ("" if repo == "makepad" else EMPTY)
        before[repo] = after[repo] = commit
    return ((tree_at(before["makepad"]), tuple((r, before[r]) for r in NESTED)),
            (tree_at(after["makepad"]), tuple((r, after[r]) for r in NESTED)))


def hybrid(commit, tip, kept):
    """The tree object of `commit` with the kept dirs from `tip`."""
    index = INDEX + ".build"
    env = dict(os.environ, GIT_INDEX_FILE=index)
    git("read-tree", commit, env=env)
    for d in kept:
        git("rm", "-r", "-q", "-f", "--cached", "--ignore-unmatch", "--", d, env=env)
        if git("cat-file", "-t", "%s:%s" % (tip, d), ok=False) == "tree":
            git("read-tree", "--prefix=%s/" % d, "%s:%s" % (tip, d), env=env)
    return git("write-tree", env=env)


def move_tree(state):
    """The second tree moves to a (makepad tree object, ((nested repository, commit or EMPTY), ..)):
    only the files that differ are written."""
    tree, nested = state
    os.makedirs(TREE, exist_ok=True)
    git("read-tree", "--reset", "-u", tree, env=dict(os.environ, GIT_INDEX_FILE=INDEX, GIT_WORK_TREE=TREE))
    for repo, commit in nested:
        side = os.path.join(TREE, repo)
        os.makedirs(side, exist_ok=True)
        ngit(repo, "read-tree", "--reset", "-u", commit, env=dict(os.environ, GIT_INDEX_FILE=index_of(repo), GIT_WORK_TREE=side))


def heads():
    """Each nested repository at its HEAD, as a tree state's second half."""
    return tuple((repo, ngit(repo, "rev-parse", "HEAD")) for repo in NESTED)


class App:
    """One app under the JIT in the second tree, hidden and muted, on the real GPU backend."""

    def __init__(self, package, log, cwd=None, env=None, args=()):
        self.log_path = log
        self.package = package
        self.file = open(log, "w")
        base = dict(os.environ, MAKEPAD_HIDE_WINDOWS="1", MAKEPAD_MUTE="1", MAKEPAD_WINDOW=os.environ.get("MAKEPAD_WINDOW", "1280x800@1"), MAKEPAD_HOME=os.path.join(WORK, "home"))
        base["MAKEPAD_REMOTE"] = "1"
        base.pop("RELAX_STD", None)
        base.update(env or {})
        self.proc = subprocess.Popen(JIT + ["run", package, *args], cwd=cwd or TREE, env=base, stdin=subprocess.DEVNULL, stdout=self.file, stderr=self.file, start_new_session=True)
        self.pid = None
        self.parked = None
        self.held = None

    def text(self):
        with open(self.log_path, errors="replace") as f:
            return f.read()

    def alive(self):
        return self.proc.poll() is None

    def ask(self, *cmd, timeout=60):
        """One inspect command in the app: (exit code, answer)."""
        try:
            r = subprocess.run([CLIENT, "inspect", str(self.pid), *cmd, "as=json"], cwd=ROOT, capture_output=True, text=True, timeout=timeout)
            return r.returncode, r.stdout if r.returncode == 0 else r.stdout + r.stderr
        except subprocess.TimeoutExpired:
            return 124, "no answer in %d s" % timeout

    def first_frames(self):
        return self.text().count("first frame at")

    def live(self, timeout=10) -> dict | None:
        """One `inspect live` JSON snapshot, or None when unavailable."""
        if not self.pid:
            return None
        code, said = self.ask("live", timeout=timeout)
        return parse_json_line(said, "held") if code == 0 else None

    def session_restart(self, timeout=10) -> dict | None:
        """The `restart` object from `inspect session`, or None when unavailable."""
        if not self.pid:
            return None
        code, said = self.ask("session", timeout=timeout)
        obj = parse_json_line(said, "restart") if code == 0 else None
        return obj.get("restart") if obj else None

    def check_held(self, since=0, live_info=None) -> str | None:
        """Sets and returns `self.held` when the host has no run and waits for a fix."""
        if self.held:
            return self.held
        text = self.text()
        line = ended_in_place(text, since)
        if line:
            self.held = line
            return self.held
        info = live_info if live_info is not None else self.live()
        if info and info.get("held") == 1:
            self.held = held_reason(info, text, since)
            return self.held
        return None

    def wait_frame(self, seen, secs, since=0, await_relaunch=False, watch_rebuild=False) -> str | None:
        """Wait for a new frame or a reason it cannot arrive. A relaunch also waits for its app check."""
        t = time.time()
        last_live = last_session = t
        while (now := time.time()) - t < secs:
            text = self.text()
            if self.pid is None:
                m = re.search(r"^\[\S+-remote\] pid=(\d+) app=", text, re.MULTILINE)
                if m:
                    self.pid = int(m.group(1))
            self.parked = parked(text, since)
            if self.parked:
                return "its JIT parked: " + self.parked
            line = ended_in_place(text, since)
            if line:
                self.held = line
                return "its run ended in place: " + line
            if await_relaunch:
                line = postponed(text, since)
                if line:
                    return "its restart was postponed: " + line
            if watch_rebuild:
                line = rebuild_failed(text, since)
                if line:
                    return "makepad rebuild failed: " + line

            got_frame = text.count("first frame at") > seen and bool(self.pid)
            live_info = None
            if self.pid and ((got_frame and await_relaunch) or now - last_live >= LIVE_POLL):
                live_info = self.live()
                last_live = now
                if live_info and live_info.get("held") == 1:
                    self.held = held_reason(live_info, text, since)
                    return "its host held: " + self.held
            if watch_rebuild and self.pid and now - last_session >= LIVE_POLL:
                rs = self.session_restart()
                last_session = now
                if rs and rs.get("phase") == "Failed":
                    return "makepad rebuild failed: " + (rs.get("failed") or "phase Failed")[:300]
            if got_frame:
                if not await_relaunch:
                    return None
                # A failed app check sets held before it clears restart_unfinished.
                if live_info is not None and live_info.get("restart_unfinished", 0) == 0 and live_info.get("held", 0) == 0:
                    return None
            if not self.alive():
                # what it said: the reason alone named nothing (makepad-fab's control app, 11-23)
                tail = text.splitlines()[-30:]
                print("live_replay: %s exited (%s) before its first frame; its log (%s) ends:\n%s" % (self.package, self.proc.returncode, self.log_path, "\n".join("  " + l[:300] for l in tail)), flush=True)
                return "exited (%s) before its first frame" % self.proc.returncode
            time.sleep(0.25)
        return "no first frame in %d s" % secs

    def stop(self):
        """Ends the app: asked first, by its recorded process group when it does not go. What a
        quit that hangs was doing is the answer (empty when it went)."""
        stuck = ""
        # A parked thread may be the one quit waits for. A held host has no app to quit.
        if self.alive() and self.pid and not self.parked and not self.held:
            code, said = self.ask("gq", timeout=30)
            t = time.time()
            while self.alive() and time.time() - t < 15:
                time.sleep(0.2)
            if self.alive():
                stuck = "gq: %s; why-hung: %s" % (said.strip()[:160], self.ask("why-hung", timeout=30)[1].strip()[:500])
        if self.alive():
            os.killpg(self.proc.pid, 9)
        self.proc.wait()
        self.file.close()
        return stuck


# "panicked" as a word ("thread 'main' (7) panicked at ..", "JIT call panicked: .."), not inside a name:
# an O3 build's note names `ScriptVmCall::resume_if_panicked` (a replay judged a clean restart a
# panic, 11-28)
BAD = re.compile(r"(?<![\w:])panicked\b|panic parked|stale access parked|internal compiler error|FATAL|fatal runtime|SIGSEGV|segfault|Bus error|the watcher stopped")
COMPILE = re.compile(r"^\s*(error(\[E\d+\])?:|.*compile error)", re.M)
# the JIT parked a thread at its call of code that did not compile (relax inspect/events.rs)
PARKED = re.compile(r"^relax: compile error parked on thread .*$", re.M)
OTHER_PARKED = re.compile(
    r"^relax: (?:panic parked on thread |stale access parked as P\d+|.* -- parked as P\d+).*$",
    re.M,
)
ENDED_IN_PLACE = re.compile(r"^relax live: the run ends in place: .*$", re.M)
BUILD_FAILED = re.compile(
    r"^makepad: (?:Compilation failed|The app's source could not be had).*$",
    re.M,
)
POSTPONED = re.compile(r"restart postponed .*$", re.M)
REBUILD_FAILED = re.compile(r"(?:rebuilding \S+ FAILED|does not start|cannot rebuild).*$")


def parked(text, since=0) -> str | None:
    """First compile-error, panic, stale-access, or update-failed park line in `text[since:]`."""
    m = PARKED.search(text, since) or OTHER_PARKED.search(text, since)
    return m.group(0)[:300] if m else None


def ended_in_place(text, since=0) -> str | None:
    """First `the run ends in place` or `Compilation failed` hold line in `text[since:]`."""
    m = ENDED_IN_PLACE.search(text, since) or BUILD_FAILED.search(text, since)
    return m.group(0)[:300] if m else None


def postponed(text, since=0) -> str | None:
    """First `restart postponed ...` line in `text[since:]`."""
    m = POSTPONED.search(text, since)
    return m.group(0)[:300] if m else None


def rebuild_failed(text, since=0) -> str | None:
    """First host-rebuild failure line in `text[since:]`."""
    for l in text[since:].splitlines():
        if "relax live" in l and REBUILD_FAILED.search(l):
            return l.strip()[:300]
    return None


def parse_json_line(text, key) -> dict | None:
    """First JSON object line in `text` containing `key`, or None."""
    for l in text.splitlines():
        if l.startswith("{") and ('"%s"' % key) in l:
            try:
                obj = json.loads(l)
                if isinstance(obj, dict) and key in obj:
                    return obj
            except ValueError:
                pass
    return None


def held_reason(live_info, text, since=0) -> str:
    """Human-readable reason why the host is held with no running app."""
    why = (live_info or {}).get("pending_why") or ""
    if why:
        return why[:300]
    line = ended_in_place(text, since)
    if line:
        return line[:300]
    err = COMPILE.search(text, since)
    if err:
        return err.group(0).strip()[:300]
    return "host held with no running app"


def bad_lines(text, since=0):
    return [l.strip()[:300] for l in text[since:].splitlines() if BAD.search(l)][:8]


# a HAL line about drawing (the GPU backends, the frame path); the media, XR-stream, input and
# window-manager services log their own news with the same prefix, and a backend's upload pool says
# when a submit had to make an arena (a hiccup that degrades gracefully, logged as such: an app's
# first frames make theirs, and every replay read that as a GPU error before its edits, 11-12), and
# the Vulkan backend says once which route a discrete GPU's first APPEND buffer takes (every
# replay's control edit read that as a GPU error, 11-27), and its memory watch says when the GPU is
# short of the UI's reserve and it asks for room (`watch_memory`: a notice of graceful degradation,
# not a failed create; a replay on a box whose GPU other work held at 21 of 24 GB read it as a GPU
# error, 11-27)
DRAW = re.compile(r"^\[hal-(?!video|media|camera|xr-remote|gamepad|dialog|wm\]|android-permission|android-dialog|vulkan\] upload arenas: |vulkan\] .* first APPEND buffer: |vulkan\] GPU memory: )")


def healthy(app, since):
    """None when the app runs, draws and answers input with nothing wrong in its log since `since`; else why."""
    if not app.alive():
        return "crash", "the process exited (%s)" % app.proc.returncode
    bad = bad_lines(app.text(), since)
    if bad:
        return ("ice" if any("internal compiler error" in b for b in bad) else "panic"), " | ".join(bad)
    app.parked = app.parked or parked(app.text(), since)
    if app.parked:
        return "parked", "its JIT parked: " + app.parked
    if app.check_held(since):
        return "held", "its host held: " + app.held
    # a frame that loses draws is an error UI, whatever a grab still shows
    draws = [l.strip()[:200] for l in app.text()[since:].splitlines() if DRAW.match(l)]
    if draws:
        # a burst during a handover or a window that stays broken: new frames tell (a hidden idle
        # window draws none by itself)
        for _ in range(3):
            app.ask("mouse", "k=move", "x=240", "y=240", "wait=1")
            app.ask("grab", "scale=0.25", "out=" + os.path.join(WORK, "grab.png"))
        text = app.text()[since:]
        later = sum(bool(DRAW.match(l)) for l in text.splitlines()) - len(draws)
        done_at = text.find("to first frame")
        first_at = text.find(draws[0])
        when = "no relaunch line" if done_at < 0 else "before the relaunch's first frame" if first_at < done_at else "after the relaunch's first frame"
        return "draw_error", "%d GPU errors, %d more over 3 forced frames, the first %s: %s" % (len(draws), later, when, draws[0])
    code, view = app.ask("session", "view")
    if code == 0 and '"over_app":true' in view.replace(" ", ""):
        return "error_view", view.strip()[:400]
    code, panics = app.ask("panics")
    if code == 0 and '"id"' in panics:
        app.parked = panics.strip()[:300]
        return "parked", panics.strip()[:400]
    code, ui = app.ask("ui")
    if code != 0 or len(ui.strip()) < 40:
        return "dead_ui", "ui: %s" % ui.strip()[:300]
    # input is answered after the next frame: the UI thread is free (a move: a click would
    # change what the app shows, which the layout checks compare)
    code, moved = app.ask("mouse", "k=move", "x=200", "y=200", "wait=1")
    if code != 0:
        return "dead_ui", "input: %s" % moved.strip()[:300]
    # and it draws: a hidden window presents on a grab (the real GPU backend)
    png = os.path.join(WORK, "grab.png")
    if os.path.exists(png):
        os.remove(png)
    code, said = app.ask("grab", "scale=0.25", "out=" + png)
    if code != 0 or not drawn(png):
        return "dead_ui", "no picture: %s" % said.strip()[:300]
    return None


def drawn(path):
    """Whether a PNG holds a picture, not one flat colour."""
    try:
        data = open(path, "rb").read()
    except OSError:
        return False
    pos, idat = 8, b""
    while pos < len(data):
        n = int.from_bytes(data[pos : pos + 4], "big")
        if data[pos + 4 : pos + 8] == b"IDAT":
            idat += data[pos + 8 : pos + 8 + n]
        pos += 12 + n
    return len(set(zlib.decompress(idat))) > 8 if idat else False


RESIZES = [(1000, 680), (1500, 940), (1280, 800)]
LAYOUTS = []


STILL = 40  # looks at the tree, half a second apart, for it to hold still (then judged as it is)


def ui_lines(app):
    """The semantic tree with rects, ids, focus and measured times dropped (they differ between runs)."""
    code, ui = app.ask("ui")
    lines = []
    for l in ui.splitlines():
        l = re.sub(r"#[0-9a-f.]+ ", "", l.rstrip())
        # (times a run measures differ from run to run)
        l = re.sub(r"\b\d+(\.\d+)? ?(ms|s)\b", "N ms", l)
        l = re.sub(r"\b(focused|hovered|pressed)\b,?", "", l)
        lines.append(l)
    return lines


def layout(app):
    """The app's layout at each of RESIZES: per size its semantic tree with rects (ids and focus
    dropped: they differ between runs) and a picture. What a patched app and a fresh start of the
    same code must agree on."""
    out = []
    for n, (w, h) in enumerate(RESIZES):
        app.ask("resize", "w=%d" % w, "h=%d" % h, "wait=1")
        settle(app)
        app.ask("mouse", "k=move", "x=5", "y=5", "wait=1")
        # the tree once it holds still: an app still loading its document (a title, a progress
        # row) lays out what it has so far, which is the moment, not the code
        lines, last = ui_lines(app), None
        for _ in range(STILL):
            if lines == last:
                break
            time.sleep(0.5)
            last, lines = lines, ui_lines(app)
        png = os.path.join(WORK, "layout-%d-%d.png" % (len(LAYOUTS), n))
        app.ask("grab", "out=" + png)
        # the drawable's size: a window whose app keeps another size than its surface draws
        # stretched and takes input offset, whatever the tree says
        try:
            head = open(png, "rb").read(24)
            lines.append("grab-surface 0,0 %dx%d" % (int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")))
        except OSError:
            lines.append("grab-surface 0,0 0x0")
        # the app's windows as it sees them (index, size, scale): a window an earlier run's state
        # was given to, or a scale another window set, shows here
        code, st = app.ask("status")
        for m in re.finditer(r'\{"i":(\d+),"t":"(?:[^"\\]|\\.)*","sz":\[([-\d.]+),([-\d.]+)\],"px":\[[-\d.]+,[-\d.]+\],"dpi":([-\d.]+)', st):
            lines.append("window-%s dpi %s 0,0 %dx%d" % (m.group(1), m.group(4), round(float(m.group(2))), round(float(m.group(3)))))
        with open(png[:-4] + ".txt", "w") as f:
            f.write("\n".join(lines) + "\n")
        out.append(((w, h), lines, png))
    LAYOUTS.append(out)
    return out


def rects(lines):
    """name -> rect of the tree's nodes (the first of a name)."""
    got = {}
    for l in lines:
        m = re.match(r"\s*(.*?)\s(-?\d+),(-?\d+) (\d+)x(\d+)", l)
        if m and m.group(1).strip() not in got:
            got[m.group(1).strip()] = tuple(int(x) for x in m.groups()[1:])
    return got


def differing(a, b):
    """Per size, the names of the nodes that moved or are in one layout only."""
    out = []
    for (_, la, _), (_, lb, _) in zip(a, b):
        ra, rb = rects(la), rects(lb)
        out.append({k for k in ra if k in rb and ra[k] != rb[k]} | (set(ra) ^ set(rb)))
    return out


def layout_diff(a, b, noise=None, than="fresh"):
    """Where a layout differs from another of the same code: the first few, or None. `noise`
    (per size, from `differing`) are the nodes a relaunch with no edit moves too: the app's state
    a restart does not keep, not a stale layout."""
    diffs = []
    for n, ((size, la, _), (_, lb, _)) in enumerate(zip(a, b)):
        ra, rb = rects(la), rects(lb)
        skip = noise[n] if noise else set()
        moved = [(k, ra[k], rb[k]) for k in ra if k in rb and ra[k] != rb[k] and k not in skip]
        only_a, only_b = sorted(set(ra) - set(rb) - skip), sorted(set(rb) - set(ra) - skip)
        if moved or only_a or only_b:
            diffs.append("%dx%d: %d of %d moved, %d only in one (%d state-only skipped)%s%s%s" % (size[0], size[1], len(moved), len(ra), len(only_a) + len(only_b), len(skip),
                "".join("; %s %s -> %s %s" % (m[0], m[1], than, m[2]) for m in moved[:3]),
                "; only here: " + ", ".join(only_a[:6]) if only_a else "", "; only in the %s: " % than + ", ".join(only_b[:6]) if only_b else ""))
    return " | ".join(diffs) or None


def settle(app):
    """The JIT idle (bounded: a busy app is judged as it is)."""
    if app.parked or app.held:
        return
    t = time.time()
    last_live = t
    while time.time() - t < SETTLE and app.alive():
        code, said = app.ask("jit", "ms=100")
        if code == 0 and "idle" in said:
            return
        now = time.time()
        if code != 0 or now - last_live >= LIVE_POLL:
            if app.check_held():
                return
            last_live = now
        time.sleep(0.5)


def tree_drift():
    """Files of the second tree that differ from what its indexes say it holds (a tree something
    else changed: then a rustc rejection says nothing about the landing): '' when none."""
    out = []
    for index, work, look in [(INDEX, TREE, git)] + [(index_of(r), os.path.join(TREE, r), lambda *a, r=r, **k: ngit(r, *a, **k)) for r in NESTED]:
        if os.path.exists(index):
            said = look("diff", "--name-only", env=dict(os.environ, GIT_INDEX_FILE=index, GIT_WORK_TREE=work), ok=False).splitlines()
            out += [os.path.relpath(os.path.join(work, f), TREE) for f in said]
    return "%d file(s) differ from the tree's index: %s" % (len(out), ", ".join(out[:5])) if out else ""


def rustc_accepts(package):
    """Whether rustc accepts the second tree as it stands (a JIT rejection is then a JIT bug)."""
    r = subprocess.run(["cargo", "check", "--release", "--offline", "-q", "-p", package], cwd=TREE, capture_output=True, text=True, env=dict(os.environ, CARGO_TARGET_DIR=os.path.join(WORK, "rustc-target")))
    drift = tree_drift() if r.returncode != 0 else ""
    return r.returncode == 0, ("(%s) " % drift if drift else "") + "\n".join(r.stderr.splitlines()[:6])


def verdict(app, seen, since):
    """The JIT's verdict on an edit: its event, or {} when the watcher never took the edit up (no
    line from it in UNSEEN s: nothing the app loaded changed)."""
    t = time.time()
    answer = ""
    while time.time() - t < VERDICT:
        code, answer = app.ask("watch", "on=patchverified,patchrefused,patchreverted,panic", "since=%d" % seen, "timeout_s=10", timeout=60)
        # (a watch that timed out answers the event `none`)
        line = next((l for l in answer.splitlines() if '"event"' in l and '"event":"none"' not in l), "")
        if line:
            ev = json.loads(line)
            if ev.get("event") == "panic":
                app.parked = ("%s: %s" % (ev.get("kind", "panic"), ev.get("message", "")))[:300]
                return {"outcome": "parked", "error": "its JIT parked: " + app.parked}
            return ev
        if not app.alive():
            return {}
        # a thread parked at code that did not compile answers no edit: the park is the verdict
        app.parked = parked(app.text(), since)
        if app.parked:
            return {"outcome": "parked", "error": "its JIT parked: " + app.parked}
        if app.check_held(since):
            return {"outcome": "held", "error": "its host held: " + app.held}
        if time.time() - t > UNSEEN and "relax live:" not in app.text()[since:]:
            return {"answer": answer.strip()[:200]} if code != 0 else {}
        if not line and "not a live session" in answer:
            time.sleep(0.25)
    return {"answer": "no verdict in %d s: %s" % (VERDICT, answer.strip()[:160])}


def rung_of(event):
    return {"patchverified": "applied", "patchreverted": "reverted"}.get(event.get("event", ""), event.get("outcome", "none"))


def control(package):
    """Nothing here counts unless an edit the app did load gets a verdict: a shard whose edits
    cannot arrive would pass everything."""
    meta = json.loads(subprocess.run(["cargo", "metadata", "--format-version", "1", "--offline", "--no-deps"], cwd=ROOT, capture_output=True, text=True, check=True).stdout)
    pkg = next(p for p in meta["packages"] if p["name"] == package)
    main = next(t["src_path"] for t in pkg["targets"] if "bin" in t["kind"])
    move_tree((git("rev-parse", "HEAD^{tree}"), heads()))
    path = os.path.join(TREE, os.path.relpath(main, ROOT))
    app = App(package, os.path.join(WORK, "control.log"))
    try:
        why = app.wait_frame(0, COLD)
        if why:
            raise SystemExit("live_replay: the control app: %s" % why)
        settle(app)
        since = len(app.text())
        code, ev = app.ask("events")
        seen = max([int(m) for m in re.findall(r'"seq":\s*(\d+)', ev)] or [0])
        with open(path, "a") as f:
            f.write("\nfn live_replay_control() -> u64 { 1 }\n")
        event = verdict(app, seen, since)
        rung = rung_of(event)
        print("live_replay: control edit: %s" % rung, flush=True)
        if rung == "none":
            print("verdict %s; asked since=%d; the app's events: %s" % (event, seen, app.ask("events", "since=0")[1][-1500:]), flush=True)
            print(app.text()[-1500:], flush=True)
            raise SystemExit("live_replay: the control edit got no verdict: this run's edits cannot arrive")
        # this tree itself starts clean (the whole log: an ICE at a fresh start is the worst kind);
        # and the checks can fail: the run's view over the app must be seen as one
        found = healthy(app, 0)
        if found:
            raise SystemExit("live_replay: the control app is not healthy: %s: %s" % found)
        app.ask("session", "view", "open")
        t = time.time()
        while time.time() - t < 30 and '"over_app":true' not in app.ask("session", "view")[1].replace(" ", ""):
            time.sleep(0.5)
        found = healthy(app, since)
        if not found or found[0] != "error_view":
            raise SystemExit("live_replay: the health check does not see the run's view over the app: %s" % (found,))
        print("live_replay: control: the health check sees a view over the app", flush=True)
    finally:
        app.stop()


def done(rec, **fields):
    rec.update(fields)
    return rec


def relaunch(app, package, what, check_tree=True):
    """The restart button and the app back healthy: None, or a (result, kind, why) to end on."""
    if app.check_held(0):
        if check_tree:
            ok, said = rustc_accepts(package)
            if not ok:
                return "skip", "tree", "rustc rejects the tree here: %s; the JIT: %s" % (said[:300], app.held[:300])
        return "fail", "restart", "%s: already held: %s" % (what, app.held)
    frames = app.first_frames()
    since = len(app.text())
    code, said = app.ask("session", "relaunch")
    if code != 0:
        return "fail", "restart", "%s: session relaunch: %s" % (what, said.strip()[:300])
    why = app.wait_frame(frames, BOOT, since, await_relaunch=True)
    if why:
        bad = bad_lines(app.text(), since)
        if bad:
            return "fail", "ice" if "internal compiler error" in bad[0] else "panic", "%s: %s" % (what, " | ".join(bad))
        if check_tree:
            ok, said = rustc_accepts(package)
            if not ok:
                return "skip", "tree", "rustc rejects the tree here: %s; the JIT: %s" % (said[:300], why[:300])
        return "fail", "restart", "%s: %s" % (what, why)
    settle(app)
    found = healthy(app, since)
    if found:
        if found[0] in ("held", "parked") and check_tree:
            ok, said = rustc_accepts(package)
            if not ok:
                return "skip", "tree", "rustc rejects the tree here: %s; the JIT: %s" % (said[:300], found[1][:300])
        return "fail", found[0], "after %s: %s" % (what, found[1])
    return None


LAYOUT_KINDS = ("stale_layout", "stale_restart")


def clock_crossed(start, end):
    """Whether a UTC or local hour, day, or local DST flag changed during the attempt."""
    ga, gb = time.gmtime(start), time.gmtime(end)
    la, lb = time.localtime(start), time.localtime(end)
    return ga[:4] != gb[:4] or la[:4] != lb[:4] or la.tm_isdst != lb.tm_isdst


def utc_stamp(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def _replay_once(sha, parent_tree, tree, package, seq):
    """One landing. The verdict record: result pass|fail|skip, rung, kind, why, seconds per step."""
    rec = {"sha": sha, "app": package, "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}}
    t0 = time.time()
    move_tree(parent_tree)
    log = os.path.join(WORK, "app-%d.log" % seq)
    app = App(package, log)
    try:
        why = app.wait_frame(0, COLD)
        rec["s"]["boot"] = round(time.time() - t0, 1)
        if why or COMPILE.search(app.text()):
            bad = bad_lines(app.text())
            ok, said = rustc_accepts(package)
            if not ok:
                # (what the JIT said about a tree rustc rejects too is recorded, not judged)
                return done(rec, result="skip", kind="tree", why="rustc rejects the parent here: %s; the JIT: %s" % (said[:300], (" | ".join(bad) or why or "")[:300]))
            if bad:
                return done(rec, result="fail", kind="ice" if "internal compiler error" in bad[0] else "panic", why="at the parent: " + " | ".join(bad))
            tail = [l for l in app.text().splitlines() if COMPILE.search(l)][:6]
            return done(rec, result="fail", kind="agreement", why="at the parent, rustc accepts: %s; %s" % (why, " | ".join(tail)))
        settle(app)
        found = healthy(app, 0)
        if found:
            ok, said = rustc_accepts(package)
            if not ok:
                return done(rec, result="skip", kind="tree", why="rustc rejects the parent here: %s; the JIT: %s: %s" % (said[:300], found[0], found[1][:300]))
            return done(rec, result="fail", kind="agreement" if found[0] == "held" else found[0], why="at the parent: " + found[1])
        noise = None
        if RESIZE:
            # the baseline: this fresh start, relaunched with no edit. What moves between them is
            # the app's own state (what a restart keeps or loses), not code: the layout checks
            # below skip those nodes
            fresh = layout(app)
            t1 = time.time()
            failed = relaunch(app, package, "the relaunch with no edit")
            rec["s"]["baseline"] = round(time.time() - t1, 1)
            if failed:
                return done(rec, result=failed[0], kind=failed[1], why="at the parent: " + failed[2])
            relaunched = layout(app)
            noise = differing(fresh, relaunched)
            rec["state_nodes"] = max(len(n) for n in noise)
            rec["state"] = layout_diff(fresh, relaunched, None, "relaunched") or ""
        # the landing arrives, as a sync moves the tree
        since = len(app.text())
        if app.check_held(0):
            ok, said = rustc_accepts(package)
            if not ok:
                return done(rec, result="skip", kind="tree", why="rustc rejects the parent here: %s; the JIT: %s" % (said[:300], app.held[:300]))
            return done(rec, result="fail", kind="agreement", why="at the parent, rustc accepts: " + app.held)
        code, ev = app.ask("events")
        seen = max([int(m) for m in re.findall(r'"seq":\s*(\d+)', ev)] or [0])
        move_tree(tree)
        t1 = time.time()
        event = verdict(app, seen, since)
        rec["s"]["verdict"] = round(time.time() - t1, 1)
        rec["rung"] = rung_of(event)
        rec["reasons"] = str(event.get("reasons", event.get("error", event.get("answer", ""))))[:400]
        settle(app)
        found = healthy(app, since)
        if found:
            if found[0] in ("parked", "held"):
                ok, said = rustc_accepts(package)
                if not ok:
                    return done(rec, result="skip", kind="tree", why="rustc rejects the tree here: %s; the JIT: %s" % (said[:300], found[1][:300]))
            return done(rec, result="fail", kind=found[0], why="after the edit (%s): %s" % (rec["rung"], found[1]))
        patched = layout(app) if rec["rung"] == "applied" and RESIZE else None
        # the restart button, on the parent's warm cache
        t2 = time.time()
        failed = relaunch(app, package, "the restart")
        rec["s"]["restart"] = round(time.time() - t2, 1)
        if failed:
            return done(rec, result=failed[0], kind=failed[1], why=failed[2])
        if RESIZE:
            restarted = layout(app)
            if patched:
                # the restart runs the same code from its start: a patched app lays out as it does
                differs = layout_diff(patched, restarted, noise, "restarted")
                if differs:
                    return done(rec, result="fail", kind="stale_layout", why="the patched app lays out unlike a restart of the same code: " + differs)
            # and the restart (the update the person clicks) lays out as a new process of the
            # same code: nothing of the previous run outlives a relaunch in place
            rec["quit"] = app.stop()
            t3 = time.time()
            app = App(package, os.path.join(WORK, "app-%d-fresh.log" % seq))
            why = app.wait_frame(0, COLD)
            rec["s"]["fresh"] = round(time.time() - t3, 1)
            if why:
                bad = bad_lines(app.text())
                if (app.held or app.parked) and not bad:
                    ok, said = rustc_accepts(package)
                    if not ok:
                        return done(rec, result="skip", kind="tree", why="rustc rejects the tree here: %s; the JIT: %s" % (said[:300], why[:300]))
                return done(rec, result="fail", kind="restart", why="a new process at the landing: %s: %s" % (why, " | ".join(bad)))
            settle(app)
            found = healthy(app, 0)
            if found:
                return done(rec, result="fail", kind=found[0], why="a new process at the landing: " + found[1])
            differs = layout_diff(restarted, layout(app), noise, "new process")
            if differs:
                return done(rec, result="fail", kind="stale_restart", why="the restarted app lays out unlike a new process of the same code: " + differs)
        if rec["rung"] == "none":
            # the watcher took it up and never answered is another thing than an edit it never saw
            rec["result"] = "silent" if event else "unreached"
        return rec
    finally:
        quit = app.stop()
        rec["quit"] = rec.get("quit") or quit
        rec["s"]["total"] = round(time.time() - t0, 1)
        # a draw error's summary quotes its first line only: its app's logs go to the lease's keep
        # dir (DEVHUB_KEEP, kept by its holder past the run), so every counted line can be read
        keep = os.environ.get("DEVHUB_KEEP")
        if rec.get("kind") == "draw_error" and keep:
            kept = []
            for path in dict.fromkeys([log, app.log_path]):
                if os.path.isfile(path):
                    os.makedirs(keep, exist_ok=True)
                    dest = os.path.join(keep, "live_replay-%s-%s" % (sha[:10], os.path.basename(path)))
                    shutil.copy2(path, dest)
                    kept.append(dest)
            if kept:
                rec["kept"] = kept


def replay(sha, parent_tree, tree, package, seq):
    """Replay a landing from scratch once if a layout mismatch spans a clock boundary."""
    t_start = time.time()
    rec = _replay_once(sha, parent_tree, tree, package, seq)
    t_end = time.time()
    if rec["result"] == "fail" and rec["kind"] in LAYOUT_KINDS and clock_crossed(t_start, t_end):
        first = rec
        rec = _replay_once(sha, parent_tree, tree, package, seq)
        rec["clock_retry"] = "%s across %s -> %s: %s" % (
            first["kind"], utc_stamp(t_start), utc_stamp(t_end), first["why"])
        if "total" in first["s"]:
            rec["s"]["attempt1"] = first["s"]["total"]
        if first.get("quit") and not rec.get("quit"):
            rec["quit"] = first["quit"]
    return rec


def wm_session():
    """A hidden WM of this tree, with a runtime dir of its own: (its app, its socket)."""
    run = os.path.join(WORK, "wm-run")
    os.makedirs(run, exist_ok=True)
    for name in os.listdir(run):
        if name.startswith("makepad-wm-"):
            try:
                os.unlink(os.path.join(run, name))
            except OSError:
                pass
    wm = App("makepad-app-wm", os.path.join(WORK, "wm.log"), cwd=ROOT, env={"XDG_RUNTIME_DIR": run}, args=["--", "--remote"])
    why = wm.wait_frame(0, COLD)
    if why:
        raise SystemExit("live_replay: the WM: %s\n%s" % (why, wm.text()[-1500:]))
    t = time.time()
    while time.time() - t < 60:
        found = [f for f in os.listdir(run) if f.startswith("makepad-wm-")]
        if found:
            return wm, os.path.join(run, found[0])
        time.sleep(0.5)
    raise SystemExit("live_replay: the WM published no socket in %s" % run)


# (what changes, the file; a comment: the crate's source identity changes, so the loader must)
HOST_EDITS = [("HAL", "platform/hal/src/lib.rs"), ("script", "platform/script/src/lib.rs"), ("makepad run", "tools/run/src/lib.rs"), ("compiler", "libs/relax/compiler/src/lib.rs")]


def host_case(app, what, path, n, rec):
    """One landing into what makepad is built from: offered, the click, its rebuild, back."""
    since = len(app.text())
    t0 = time.time()
    with open(os.path.join(ROOT, path), "a") as f:
        f.write("\n// live_replay host edit %d\n" % n)
    rec["what"] = what
    last_live = t0
    while True:
        text = app.text()
        app.parked = parked(text, since)
        if app.parked:
            return done(rec, result="fail", kind="parked", why="its JIT parked: " + app.parked)
        now = time.time()
        if now - last_live >= LIVE_POLL:
            if app.check_held(since):
                return done(rec, result="fail", kind="held", why="its host held: " + app.held)
            last_live = now
        said = text[since:]
        if "a restart applies it" in said:
            break
        if "FAILED" in said or "does not start" in said or "cannot rebuild" in said:
            return done(rec, result="fail", kind="rebuild", why=" | ".join(l[:240] for l in said.splitlines() if "relax live" in l)[-700:])
        if not app.alive():
            return done(rec, result="fail", kind="crash", why="exited (%s) while makepad rebuilt: %s" % (app.proc.returncode, said[-400:]))
        if "is unchanged" in said:
            return done(rec, result="unreached", rung="unchanged", why="makepad did not change")
        if now - t0 > REBUILD:
            return done(rec, result="silent", why="no restart offered in %d s: %s" % (REBUILD, said[-400:]))
        time.sleep(1)
    # the offer comes first: the build is the click's, or the next `cargo run`'s
    if "it builds first in the background" in app.text()[since:]:
        return done(rec, result="fail", kind="rebuild", why="makepad built before anyone asked")
    rec["s"]["offered"] = round(time.time() - t0, 1)
    rec["rung"] = "restart"
    found = healthy(app, since)
    if found:
        return done(rec, result="fail", kind=found[0], why="while offered: " + found[1])
    pid, frames = app.pid, app.first_frames()
    since = len(app.text())
    t1 = time.time()
    code, said = app.ask("session", "relaunch")
    if code != 0:
        return done(rec, result="fail", kind="restart", why="session relaunch: " + said.strip()[:300])
    why = app.wait_frame(frames, REBUILD + BOOT, since, watch_rebuild=True)
    rec["s"]["restart"] = round(time.time() - t1, 1)
    if why:
        return done(rec, result="fail", kind="restart", why="the click: %s: %s" % (why, " | ".join(bad_lines(app.text(), since)) or app.text()[-400:]))
    if app.pid != pid:
        return done(rec, result="fail", kind="restart", why="the process changed: %s -> %s" % (pid, app.pid))
    settle(app)
    found = healthy(app, since)
    if found:
        return done(rec, result="fail", kind=found[0], why="after the click: " + found[1])
    kept = re.search(r"restart \(.*?\): kept (\d+) window frame", app.text()[since:])
    if not kept or int(kept.group(1)) < 1:
        return done(rec, result="fail", kind="state", why="no window frame handed over: %s" % (kept.group(0) if kept else "no `kept` line"))
    rec["state"] = kept.group(0)[:160]
    return rec


def _host_once(package, wm, allow_clock_retry):
    os.makedirs(WORK, exist_ok=True)
    # the edits go back once the run ends: left in the tree, they read as the tree's own changes
    # (a lease's end saved them as unlanded work, 11-14)
    kept = {}
    for _, path in HOST_EDITS:
        with open(os.path.join(ROOT, path)) as source:
            kept[path] = source.read()
    t_start = time.time()
    clock_retry = None
    env, session = {}, None
    if wm:
        session, socket = wm_session()
        env = {"MAKEPAD_WM_SOCKET": socket, "MAKEPAD_TEST_LANE": os.environ.get("MAKEPAD_TEST_LANE", "jitupdate")}
    app = App(package, os.path.join(WORK, "host.log"), cwd=ROOT, env=env)
    tally, recs = {}, []
    try:
        why = app.wait_frame(0, COLD)
        if why:
            raise SystemExit("live_replay: %s: %s\n%s" % (package, why, app.text()[-1500:]))
        settle(app)
        found = healthy(app, 0)
        if found:
            raise SystemExit("live_replay: %s is not healthy before the edits: %s: %s" % (package, *found))
        noise = None
        if RESIZE:
            # the baseline: what a relaunch with no edit moves is the app's state (see replay)
            fresh = layout(app)
            failed = relaunch(app, package, "the relaunch with no edit", check_tree=False)
            if failed:
                raise SystemExit("live_replay: %s: %s" % (package, failed[2]))
            noise = differing(fresh, layout(app))
        for n, (what, path) in enumerate(HOST_EDITS):
            rec = host_case(app, what, path, n, {"app": package, "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}})
            recs.append(rec)
            tally[rec["result"]] = tally.get(rec["result"], 0) + 1
            print("%-12s %-9s %-9s %-10s %s %s %s" % (what, rec["result"], rec["rung"], rec["kind"], rec["s"], rec["why"], rec.get("state", "")), flush=True)
            if rec["result"] == "fail" and not app.alive():
                break
        if RESIZE and app.alive() and not tally.get("fail"):
            # each restart into a rebuilt makepad replaced the process (exec, its state carried
            # over): after them the app lays out as a new process of the same code
            restarted = layout(app)
            app.stop()
            app = App(package, os.path.join(WORK, "host-fresh.log"), cwd=ROOT, env=env)
            why = app.wait_frame(0, COLD)
            rec = {"app": package, "what": "new process", "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}}
            if why:
                rec.update(result="fail", kind="restart", why="a new process after %d restarts into a rebuilt makepad: %s" % (len(recs), why))
            else:
                differs = layout_diff(restarted, layout(app), noise, "new process")
                t_end = time.time()
                if differs:
                    if allow_clock_retry and clock_crossed(t_start, t_end):
                        clock_retry = "stale_restart across %s -> %s: %s" % (utc_stamp(t_start), utc_stamp(t_end), differs)
                    else:
                        rec.update(result="fail", kind="stale_restart", why="after %d restarts into a rebuilt makepad the app lays out unlike a new process: %s" % (len(recs), differs))
            if not clock_retry:
                tally[rec["result"]] = tally.get(rec["result"], 0) + 1
                print("%-12s %-9s %-9s %-10s %s" % ("new process", rec["result"], rec["rung"], rec["kind"], rec["why"]), flush=True)
    finally:
        said = [l[:240] for l in app.text().splitlines() if re.search(r"relax live|relaunch|panic", l)]
        app.stop()
        if session:
            print("the WM's log: " + " | ".join(session.text().splitlines()[-4:])[:500], flush=True)
            session.stop()
        for path, text in kept.items():
            with open(os.path.join(ROOT, path), "w") as f:
                f.write(text)
    if clock_retry:
        return None, clock_retry
    if tally.get("fail") or tally.get("silent"):
        print("\n".join(said[-30:]), flush=True)
    return rate(package + (" under the WM" if wm else "") + " rebuild", tally, {}), None


def host(package, wm):
    code, retry = _host_once(package, wm, allow_clock_retry=True)
    if retry is not None:
        print("live_replay: %s rebuild crossed a clock boundary, retrying once: %s" % (package, retry[:1500]), flush=True)
        code, _ = _host_once(package, wm, allow_clock_retry=False)
    return code


def print_pair(landing):
    if len(landing.commits) > 1:
        label = "paired" if landing.land_id is None else "landing #%s" % landing.land_id
        print("  %s: %s" % (label, " ".join("%s=%s" % (r, c[:10]) for r, c in landing.commits)), flush=True)


def chain(package, steps):
    """--chain: one app takes the landings one after another with no restart between them, as a
    person's running app takes a day of landings (the restarts they need wait). Each edit's
    verdict is judged: a refusal of a tree rustc accepts is a JIT bug (refused_valid: the edit was
    checked against what the running program lacks, edits that wait for a restart)."""
    if not steps:
        return rate(package + " chain", {}, {})
    move_tree(steps[0][1])
    app = App(package, os.path.join(WORK, "chain.log"))
    tally, rungs = {}, {}
    try:
        why = app.wait_frame(0, COLD)
        if why:
            raise SystemExit("live_replay: the chain's app: %s" % why)
        settle(app)
        found = healthy(app, 0)
        if found:
            raise SystemExit("live_replay: the chain's app is not healthy at its start: %s: %s" % found)
        for landing, _, state in steps:
            sha = primary_sha(landing)
            rec = {"sha": sha, "app": package, "result": "pass", "rung": "-", "kind": "", "why": "", "s": {}}
            since = len(app.text())
            code, ev = app.ask("events")
            seen = max([int(m) for m in re.findall(r'"seq":\s*(\d+)', ev)] or [0])
            move_tree(state)
            t = time.time()
            event = verdict(app, seen, since)
            rec["s"]["verdict"] = round(time.time() - t, 1)
            rec["rung"] = rung_of(event)
            rec["reasons"] = str(event.get("reasons", event.get("error", event.get("answer", ""))))[:400]
            settle(app)
            found = healthy(app, since)
            if found and found[0] in ("parked", "held") and not rustc_accepts(package)[0]:
                done(rec, result="skip", kind="tree", why="rustc rejects the tree here; the JIT: " + found[1][:300])
            elif found:
                done(rec, result="fail", kind=found[0], why=found[1])
            elif rec["rung"] == "refuse":
                ok, said = rustc_accepts(package)
                if ok:
                    done(rec, result="fail", kind="refused_valid", why="rustc accepts the tree the JIT refused: " + rec["reasons"])
            tally[rec["result"]] = tally.get(rec["result"], 0) + 1
            rungs[rec["rung"]] = rungs.get(rec["rung"], 0) + 1
            print("%s %-9s %-9s %-13s %s %s %s" % (sha[:10], rec["result"], rec["rung"], rec["kind"], rec["s"], rec["why"][:600], rec["reasons"] if rec["result"] != "fail" else ""), flush=True)
            print_pair(landing)
            if not app.alive() or app.held:
                break
    finally:
        app.stop()
    return rate(package + " chain", tally, rungs)


def rate(name, tally, rungs):
    """The run's one number."""
    judged = tally.get("pass", 0) + tally.get("fail", 0) + tally.get("silent", 0)
    pct = "%.1f%%" % (100.0 * tally.get("pass", 0) / judged) if judged else "n/a"
    print("live_replay: %s pass rate %s (%d/%d judged; %d fail, %d silent, %d skip, %d unreached; rungs %s)" % (name, pct, tally.get("pass", 0), judged, tally.get("fail", 0), tally.get("silent", 0), tally.get("skip", 0), tally.get("unreached", 0), rungs), flush=True)
    if not judged:
        return 1
    return 3 if tally.get("fail") or tally.get("silent") else 0


def main():
    os.makedirs(WORK, exist_ok=True)
    package = opt("--app", "makepad-app-storybook")
    subprocess.run(["cargo", "build", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad"], cwd=ROOT, check=True)
    shutil.copy2(os.path.join(os.environ.get("CARGO_TARGET_DIR", os.path.join(ROOT, "target")), "host", "makepad"), CLIENT)
    if "--host" in sys.argv:
        sys.exit(host(package, "--wm" in sys.argv))
    landings_path = opt("--landings", "")
    if landings_path:
        land_index = load_landings(landings_path)
        print(loaded_line(land_index, landings_path), flush=True)
    else:
        land_index = None
        print("live_replay: no --landings given; pairing cross-repo lands from git", flush=True)
    count = int(opt("--count", "300"))
    tip = git("rev-parse", "HEAD")
    history = [x for x in opt("--history", "").split(",") if x]
    base = history[0] if history else tip
    tips = dict(heads())
    for x in history[1:]:
        repo, _, commit = x.partition("=")
        if repo not in tips or not commit:
            raise SystemExit("live_replay: --history %s: no nested repository %s here (%s)" % (x, repo, " ".join(NESTED) or "none"))
        tips[repo] = commit
    only = [x for x in opt("--only", "").split(",") if x]
    meta = metadata()
    kept = kept_dirs(meta)
    all_tips = {"makepad": base, **tips}
    if only:
        items = resolve_only(only, all_tips, land_index)
    else:
        items = landings(meta, package, kept, base, tips, count, land_index)[::-1]
    print("live_replay: %s, %d landings, kept at this tree: %s" % (package, len(items), " ".join(kept)), flush=True)
    print("live_replay: the second tree holds makepad and %s" % (" ".join(NESTED) or "no nested repository"), flush=True)
    results = os.path.join(WORK, "results-%s.jsonl" % package)
    drifted = 0
    tally = {}
    rungs = {}
    trees = {}

    def tree_at(commit):
        if commit not in trees:
            trees[commit] = hybrid(commit, tip, kept)
        return trees[commit]

    parent_cache = {}

    if "--chain" in sys.argv:
        steps = [(item, *resolve_states(item, all_tips, tree_at, land_index, parent_cache))
                 for item in sorted(items, key=landing_sort_key)]
        os.makedirs(TREE, exist_ok=True)
        shutil.copy2(os.path.join(ROOT, "Cargo.lock"), os.path.join(TREE, "Cargo.lock"))
        sys.exit(chain(package, steps))
    if items:
        control(package)
    # (the lockfile is not committed: the second tree resolves as this one does)
    os.makedirs(TREE, exist_ok=True)
    shutil.copy2(os.path.join(ROOT, "Cargo.lock"), os.path.join(TREE, "Cargo.lock"))
    with open(results, "w") as out:
        for seq, landing in enumerate(items):
            sha = primary_sha(landing)
            a, b = resolve_states(landing, all_tips, tree_at, land_index, parent_cache)
            if a == b:
                rec = {"sha": sha, "app": package, "result": "skip", "rung": "-", "kind": "kept", "why": "only the JIT's and the providers' files changed", "s": {}}
            else:
                rec = replay(sha, a, b, package, seq)
                if rec["result"] in ("fail", "silent"):
                    with open(os.path.join(WORK, "app-%d.log" % seq), errors="replace") as f:
                        # what the JIT said (a draw error repeated each frame buries it otherwise)
                        lines = f.read().splitlines()
                        said = [l[:300] for l in lines if re.search(r"relax|relaunch|error|panic", l)]
                        rec["log"] = "\n".join(said[-40:]) + "\n(%d lines, %d draw errors)" % (len(lines), sum(bool(DRAW.match(l)) for l in lines))
            rec["commits"] = dict(landing.commits)
            if landing.land_id is not None:
                rec["land_id"] = landing.land_id
            tally[rec["result"]] = tally.get(rec["result"], 0) + 1
            rungs[rec["rung"]] = rungs.get(rec["rung"], 0) + 1
            drifted = drifted + 1 if rec["kind"] == "tree" else 0
            out.write(json.dumps(rec) + "\n")
            out.flush()
            if rec.get("quit"):
                print("%s quit hangs: %s" % (sha[:10], rec["quit"]), flush=True)
            print("%s %-9s %-9s %-10s %s %s %s" % (sha[:10], rec["result"], rec["rung"], rec["kind"], rec["s"], rec["why"], rec.get("reasons", "") if rec["rung"] not in ("applied", "none") or rec["result"] == "silent" else ""), flush=True)
            print_pair(landing)
            if rec.get("state"):
                print("  the relaunch with no edit moved (skipped as state): %s" % rec["state"][:1500], flush=True)
            if rec.get("log"):
                print(rec["log"], flush=True)
            if rec.get("clock_retry"):
                print("  retried once after a clock boundary: %s" % rec["clock_retry"][:1500], flush=True)
            if rec.get("kept"):
                print("  its app's logs, kept: %s" % ", ".join(rec["kept"]), flush=True)
            if drifted >= DRIFTED:
                print("live_replay: the tree no longer builds against this JIT from %s back: %d of %d landings not run" % (sha[:10], len(items) - seq - 1, len(items)), flush=True)
                break
    sys.exit(rate(package, tally, rungs))


if __name__ == "__main__":
    main()
