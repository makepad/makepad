#!/usr/bin/env python3
"""Rolling, read-only verification of work/commercial-main pairs.

The controller runs beside the repos; all compilation/execution is leased.
--once --pair W C --dry runs a throwaway pair without changing the ledgers.
A first run needs no seed: a passing tip establishes the verified baseline.
Failure before that is recorded as unbisectable, never attributed to a guess.
"""
import argparse
from contextlib import ExitStack
from datetime import datetime, timezone
import fcntl
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

CHILDREN = set()


def log(message):
    print(f"[{datetime.now(timezone.utc).isoformat(timespec='seconds')}] {message}", flush=True)


def git(repo, *args):
    return subprocess.check_output(["git", "-C", str(repo), *args], text=True).strip()


def stamp():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def append(path, text):
    with path.open("a") as stream:
        stream.write(text + "\n")
        stream.flush()
        os.fsync(stream.fileno())


def stop(signum, _frame):
    # Only the devhub clients we started; their leases own the node processes.
    for child in list(CHILDREN):
        if child.poll() is None:
            child.terminate()
    for child in list(CHILDREN):
        child.wait()
    raise SystemExit(128 + signum)


def read_pairs(path):
    if not path.exists():
        return []
    return [tuple(row.split()[1:3]) for row in path.read_text().splitlines()
            if len(row.split()) >= 3 and all(re.fullmatch(r"[0-9a-f]{40}", s) for s in row.split()[1:3])]


def snapshot(repo, commercial):
    # Do not capture the half of a cooperating land-batch fast-forward. No lock
    # is taken in either source repo, and neither ref is ever written here.
    common = Path(git(repo, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    lock = common / "land-ff.lock"
    if lock.exists():
        return None
    pair = (git(repo, "rev-parse", "refs/heads/work"), git(commercial, "rev-parse", "refs/heads/main"))
    again = (git(repo, "rev-parse", "refs/heads/work"), git(commercial, "rev-parse", "refs/heads/main"))
    return pair if pair == again and not lock.exists() else None


def first_parent(repo, base, tip):
    chain = git(repo, "rev-list", "--first-parent", tip).splitlines()
    if base not in chain:
        raise ValueError(f"baseline {base} is not on the first-parent history of {tip}")
    return list(reversed(chain[:chain.index(base)]))


def landing_events(repo, ref, chain, side):
    """Assign each first-parent commit the time its batch became visible.

    Commit dates are not landing dates (branches may be old or gated out of
    order). A reflog ff can contain several merges; keep their first-parent
    order and work-before-commercial on equal timestamps, like land-batch.
    """
    if not chain:
        return []
    positions = {sha: i for i, sha in enumerate(chain)}
    rows = git(repo, "reflog", "show", "--format=%H %gd", "--date=raw", ref).splitlines()
    events, covered = [], -1
    for row in reversed(rows):
        sha = row.split()[0]
        pos = positions.get(sha, -1)
        match = re.search(r"@\{([0-9]+) [+-][0-9]+\}", row)
        if pos > covered and match:
            for index in range(covered + 1, pos + 1):
                events.append((int(match[1]), side, index, chain[index]))
            covered = pos
    if covered != len(chain) - 1:
        raise ValueError(f"{ref} reflog does not cover the pinned first-parent range")
    return events


class Verifier:
    def __init__(self, args):
        self.args = args
        self.repo = args.repo.resolve()
        self.commercial = self.repo / "apps/commercial"
        self.state = args.state_dir.resolve()
        self.state.mkdir(parents=True, exist_ok=True)
        self.verified = self.state / "verified-tips.txt"
        self.failures = self.state / "verify-failures.txt"
        self.runs = self.state / "verify-runs"
        self.runs.mkdir(exist_ok=True)
        self.serial = 0

    def gate(self, pair):
        """0 = all four verdicts OK; 1 = gate failure; 2 = incomplete evidence."""
        self.serial += 1
        directory = self.runs / f"{stamp().replace(':', '')}-{os.getpid()}-{self.serial}"
        directory.mkdir()
        (directory / "pair.txt").write_text(" ".join(pair) + "\n")
        rev = f"work@{pair[0]},main@{pair[1]}"
        # Runtime legs contain timing assertions: exclusive timed leases give
        # each half a different Linux node while they overlap. Platform checks
        # have no runtime timings. ARM64 diff runs under a timed Mac lease.
        legs = (
            ("full-a", "timed", "arch=x64,os=linux,role=pool", "NODEGATE", ["bash", "tools/run/fixture/land_gate.sh", "full-a"]),
            ("full-b", "timed", "arch=x64,os=linux,role=pool", "NODEGATE", ["bash", "tools/run/fixture/land_gate.sh", "full-b"]),
            ("platforms", "build", "arch=arm64,os=macos", "PLATFORMS", ["bash", "tools/run/fixture/check_platforms.sh"]),
            ("arm64", "timed", "arch=arm64,os=macos", "ARM64", ["bash", "tools/run/fixture/arm64_leg.sh", "--diff-fast"]),
        )
        log(f"VERIFY pair {' '.join(pair)} logs={directory}")
        results = []
        with ExitStack() as stack:
            pending = []
            for name, kind, needs, marker, command in legs:
                output = stack.enter_context((directory / f"{name}.log").open("w"))
                # Check the pair again on the node before any gate command.
                guarded = ["bash", "-c", 'test "$(git rev-parse HEAD)" = "$1" && test "$(git -C apps/commercial rev-parse HEAD)" = "$2" || exit 2; shift 2; exec nice -n 10 env CARGO_INCREMENTAL=0 "$@"', "verify-tip", *pair, *command]
                child = subprocess.Popen(
                    [str(self.args.devhub), "devhub", "lease", kind, "--needs", needs,
                     "--lane", f"{self.args.lane}-{name}", "--rev", rev, "--", *guarded],
                    cwd=self.repo, stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
                )
                CHILDREN.add(child)
                pending.append((name, marker, child))
            # Nonblocking polling lets us report progress without losing other
            # leg completions behind whichever child was launched first.
            last_report = time.monotonic()
            while pending:
                for name, marker, child in list(pending):
                    rc = child.poll()
                    if rc is None:
                        continue
                    CHILDREN.discard(child)
                    pending.remove((name, marker, child))
                    text = (directory / f"{name}.log").read_text(errors="replace")
                    verdicts = re.findall(rf"^{marker}-(OK|FAIL)\b.*$", text, re.M)
                    # A missing platform SDK is blocked coverage, not a pass.
                    skipped = name == "platforms" and "not installed here (skipped):" in text
                    result = (2 if not verdicts or skipped else
                              1 if verdicts[-1] == "FAIL" else
                              0 if rc == 0 else 2)
                    results.append(result)
                    summary = next((line for line in reversed(text.splitlines()) if line.startswith(marker + "-")), f"no verdict (exit {rc})")
                    log(f"{name}: {'INCOMPLETE' if result == 2 else 'OK' if result == 0 else 'FAIL'} {summary}")
                if pending:
                    if time.monotonic() - last_report >= 60:
                        log("waiting: " + ", ".join(item[0] for item in pending))
                        last_report = time.monotonic()
                    time.sleep(1)
        result = 2 if 2 in results else int(1 in results)
        log(f"VERIFY {'OK' if result == 0 else 'FAIL' if result == 1 else 'INCOMPLETE'}: {results.count(0)}/4 legs passed")
        return result

    def record_good(self, pair):
        if not self.args.dry and pair not in read_pairs(self.verified):
            append(self.verified, f"{stamp()} {' '.join(pair)}")

    def bisect(self, tip):
        baseline = None
        for pair in reversed(read_pairs(self.verified)):
            try:
                work = first_parent(self.repo, pair[0], tip[0])
                commercial = first_parent(self.commercial, pair[1], tip[1])
                baseline = pair
                break
            except ValueError:
                continue
        if baseline is None:
            return "unresolved=no-verified-first-parent-baseline"
        try:
            events = sorted(landing_events(self.repo, "work", work, 0) + landing_events(self.commercial, "main", commercial, 1))
        except ValueError as error:
            return f"unresolved={error}"
        pairs = [baseline]
        for _, side, _, sha in events:
            pair = list(pairs[-1])
            pair[side] = sha
            pairs.append(tuple(pair))
        if pairs[-1] != tip or len(pairs) < 2:
            return "unresolved=empty-or-inconsistent-first-parent-range"
        # Reconfirm the baseline in today's environment before blaming code.
        if self.gate(baseline) != 0:
            return "unresolved=verified-baseline-no-longer-passes"
        low, high = 0, len(pairs) - 1
        while high - low > 1:
            middle = (low + high) // 2
            result = self.gate(pairs[middle])
            if result == 2:
                return f"unresolved=incomplete-bisect-pair {' '.join(pairs[middle])}"
            if result == 0:
                self.record_good(pairs[middle])
                low = middle
            else:
                high = middle
        _, side, _, sha = events[high - 1]
        repo = self.repo if side == 0 else self.commercial
        subject = git(repo, "show", "-s", "--format=%s", sha)
        # Like git bisect, this assumes one good -> bad transition in the range.
        return f"first-bad={'work' if side == 0 else 'commercial'}:{sha} good={'/'.join(pairs[low])} bad={'/'.join(pairs[high])} order=first-parent/reflog subject={subject}"

    def run(self):
        # flock is available through Python on macOS too. The file stays; the
        # kernel drops the lock on exit/crash, so there is no stale mkdir lock.
        with (self.state / "verify-tip.lock").open("a") as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                sys.exit("another verify-tip holds this state directory")
            attempted = set(read_pairs(self.failures)) if not self.args.dry else set()
            while True:
                pair = tuple(self.args.pair) if self.args.pair else snapshot(self.repo, self.commercial)
                if pair is None or (not self.args.pair and pair in read_pairs(self.verified)) or pair in attempted:
                    if self.args.once:
                        log("nothing new to verify")
                        return 0
                    time.sleep(60)
                    continue
                result = self.gate(pair)
                if result == 0:
                    self.record_good(pair)
                elif result == 1:
                    diagnosis = self.bisect(pair)
                    log(f"VERIFY-FAIL {' '.join(pair)} {diagnosis}")
                    if not self.args.dry:
                        append(self.failures, f"{stamp()} {' '.join(pair)} {diagnosis}")
                    attempted.add(pair)
                # Incomplete infrastructure evidence is retried; a source
                # failure is remembered so the daemon can follow newer tips.
                if self.args.once:
                    return result
                if result == 2:
                    time.sleep(60)


def main():
    root = Path(__file__).resolve().parents[3]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path("/Users/admin/makepad"))
    parser.add_argument("--devhub", type=Path, default=Path("/Users/admin/makepad/target/host/makepad"))
    parser.add_argument("--state-dir", type=Path, default=root / "local/agent_state/relax-campaign")
    parser.add_argument("--lane", default="landverify")
    parser.add_argument("--once", action="store_true")
    parser.add_argument("--pair", nargs=2, metavar=("WORK_SHA", "COMMERCIAL_SHA"))
    parser.add_argument("--dry", action="store_true", help="run real gates, do not write verified/failure ledgers")
    args = parser.parse_args()
    if args.pair and not args.once:
        parser.error("--pair requires --once")
    if args.pair and not all(re.fullmatch(r"[0-9a-f]{40}", sha) for sha in args.pair):
        parser.error("--pair requires two full commit SHAs")
    if not re.fullmatch(r"[a-zA-Z0-9_-]+", args.lane):
        parser.error("invalid lane name")
    os.environ.setdefault("DEVHUB_AGENT", "landfast-verifier")
    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, stop)
    verifier = Verifier(args)
    try:
        return verifier.run()
    finally:
        for child in list(CHILDREN):
            if child.poll() is None:
                child.terminate()
        for child in list(CHILDREN):
            child.wait()


if __name__ == "__main__":
    sys.exit(main())
