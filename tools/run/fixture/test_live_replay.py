"""Regression tests for live_replay.py."""

import importlib.util
import io
import json
import sys
import time
import unittest
from contextlib import ExitStack, redirect_stdout
from datetime import datetime, timezone
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest.mock import Mock, call, patch


sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("live_replay", Path(__file__).with_name("live_replay.py"))
live = importlib.util.module_from_spec(spec)
spec.loader.exec_module(live)


def utc(day, hour, minute, second):
    return datetime(2026, 11, day, hour, minute, second, tzinfo=timezone.utc).timestamp()


MIDNIGHT = (utc(27, 23, 59, 18), utc(28, 0, 0, 4))
SAME_HOUR = (utc(28, 0, 5, 0), utc(28, 0, 5, 45))


def record(result, kind="", rung="none", why="", **extra):
    return dict(result=result, kind=kind, rung=rung, why=why, s={}, **extra)


def days(count):
    return [((800, 600), ['Label "%d days" @ 20,30 80x20' % count], None)]


class LandingTests(unittest.TestCase):
    apps = "apps/apps"
    commercial = "apps/commercial"
    library = "apps/source-library"
    makepad = "8b6ce4e23b".ljust(40, "0")
    fab = "cd21317c11".ljust(40, "0")
    makepad_pre = "69f2c2a3ea".ljust(40, "0")
    fab_pre = "0fe47b87".ljust(40, "0")
    commercial_pre = "46f11f05d2".ljust(40, "0")
    timestamp = 1795807541

    def setUp(self):
        scope = ExitStack()
        self.addCleanup(scope.close)
        self.root = Path(scope.enter_context(TemporaryDirectory()))
        self.nested = [self.apps]
        self.answers = {}
        self.ancestor = Mock(return_value=True)
        scope.enter_context(patch.multiple(live, ROOT=str(self.root), NESTED=self.nested,
                                          git=lambda *a, **kw: self.git("makepad", *a, **kw),
                                          ngit=self.git, is_ancestor=self.ancestor))

    def git(self, repo, *args, **kwargs):
        key = (repo, args)
        if key not in self.answers:
            if args[0] == "rev-parse" and args[-1].endswith("^{commit}") and not kwargs["ok"]:
                return ""
            self.fail("unexpected Git query in %s: %s" % (repo, args))
        return self.answers[key]

    @property
    def tips(self):
        return dict.fromkeys(["makepad", *self.nested], "HEAD")

    def log(self, repo, rows, *args):
        self.answers[repo, ("log", "--first-parent", "--format=%ct\t%H\t%s", *args)] = "\n".join(
            "%d\t%s\t%s" % row for row in rows).strip()

    def scan(self, start, end, by_repo):
        for repo in self.tips:
            self.log(repo, by_repo.get(repo, []), "--since=%d" % (start - 5), "--until=%d" % (end + 5), "HEAD")

    def rev(self, repo, rev, answer):
        self.answers[repo, ("rev-parse", "--verify", "-q", rev)] = answer

    def at(self, repo, t, upper, answer):
        self.answers[repo, ("log", "--first-parent", "-1", "--format=%H", "--before=%d" % t, upper)] = answer

    def touching(self, by_repo, index=None):
        dirs = []
        for repo, rows in by_repo.items():
            dirs.append("crate" if repo == "makepad" else repo + "/crate")
            self.log(repo, rows, "-10", "HEAD", "--", "crate")
        with patch.object(live, "closure", return_value=dirs):
            return live.landings({}, "app", [], "HEAD", dict.fromkeys(self.nested, "HEAD"), 10, index)

    def index(self, *rows):
        path = self.root / "landings.jsonl"
        path.write_text("\n" + "\n\n".join(json.dumps(row) for row in rows))
        return live.load_landings(str(path))

    def fab_record(self):
        return dict(id=10593, t=self.timestamp + 1, commits={self.apps: self.fab, "makepad": self.makepad},
                    pre={"makepad": self.makepad_pre, self.commercial: self.commercial_pre},
                    post={"makepad": self.makepad, self.commercial: self.commercial_pre})

    @staticmethod
    def subject(branch, target="main"):
        return "Merge branch '%s' into %s (devhub land)" % (branch, target)

    def states(self, item, index=None):
        return live.resolve_states(item, self.tips, lambda sha: "hybrid-" + sha, index)

    def test_load_landings_and_group_cross_repo_record(self):
        self.nested.append(self.commercial)
        index = self.index(self.fab_record())
        path = str(self.root / "landings.jsonl")
        loaded = "live_replay: loaded 1 land records from " + path
        self.assertIsNone(index.trimmed, "a full export must have no trim metadata")
        self.assertEqual(live.loaded_line(index, path), loaded, "a full export must keep the original load message")
        marker = dict(window=1, older=2, newer=0, total=3)
        index = self.index(dict(self.fab_record(), trimmed=marker))
        self.assertEqual(index.trimmed, marker, "the index must retain all four counts from the first row")
        self.assertEqual(live.loaded_line(index, path),
                         loaded + " (the master's 1-row window left out 2 older and 0 newer of 3; the oldest kept is #10593)",
                         "the load message must identify the window, omitted rows, total, and oldest kept land")
        commits = {"makepad": [(self.timestamp, self.makepad, self.subject("rc-fabvprint", "work"))],
                   self.apps: [(self.timestamp, self.fab, self.subject("rc-fabvprint"))]}
        items = self.touching(commits, index)
        self.assertEqual(len(items), 1, "two touching halves of one land must replay once")
        land = items[0]
        self.assertEqual((land.land_id, land.order, live.primary_sha(land)), (10593, 0, self.makepad),
                         "the record must retain its land number, position and makepad primary")
        self.assertEqual(land.commits, (("makepad", self.makepad), (self.apps, self.fab)), "both halves must move")
        self.rev(self.apps, self.fab + "^1", self.fab_pre)
        self.assertEqual(self.states(land, index), (
            ("hybrid-" + self.makepad_pre, ((self.apps, self.fab_pre), (self.commercial, self.commercial_pre))),
            ("hybrid-" + self.makepad, ((self.apps, self.fab), (self.commercial, self.commercial_pre)))),
            "the parent must use Fab's first parent and both states must pin commercial")

    def test_land_record_includes_untouched_closure_half_diff_branch_and_large_dt(self):
        self.nested.append(self.commercial)
        m0, m1, m2, a0, a1, c0, c1, c2 = ("%040x" % i for i in range(1, 9))
        index = self.index(
            dict(id=9399, t=1019, commits={"makepad": m1, self.apps: a1, self.commercial: c1},
                 pre={"makepad": m0, self.commercial: c0}),
            dict(id=9716, t=2000, commits={"makepad": m2, self.commercial: c2},
                 pre={"makepad": m1, self.commercial: c1}))
        # Only the apps merge at +19 s and the public rc-voxcar-iso merge touch the app.
        rows = {self.apps: [(1019, a1, self.subject("rc-repomove"))],
                "makepad": [(2000, m2, self.subject("rc-voxcar-iso", "work"))]}
        self.scan(1000, 2000, {"makepad": [(1000, m1, self.subject("rc-repomove", "work"))],
                               self.commercial: [(1000, c1, self.subject("rc-repomove")),
                                                 (2000, c2, self.subject("rc-voxcar-iso2"))]})
        items = self.touching(rows, index)
        self.assertEqual([l.land_id for l in items], [9399, 9716], "each touching record must replay once")
        self.rev(self.apps, a1 + "^1", a0)
        self.at(self.apps, 2000, "HEAD", a1)
        self.assertEqual(self.states(items[0], index), (
            ("hybrid-" + m0, ((self.apps, a0), (self.commercial, c0))),
            ("hybrid-" + m1, ((self.apps, a1), (self.commercial, c1)))),
            "a delayed third half must move with the two untouched halves")
        self.assertEqual(self.states(items[1], index), (
            ("hybrid-" + m1, ((self.apps, a1), (self.commercial, c1))),
            ("hybrid-" + m2, ((self.apps, a1), (self.commercial, c2)))),
            "the land record must pair different branch names while holding apps fixed")
        self.ancestor.side_effect = lambda r, c, tip: c != c1
        self.assertEqual([l.land_id for l in self.touching(rows, index)], [9716],
                         "an unreachable commercial half must exclude the entire land")

    def test_states_unchanged_nested_repo_never_pulls_same_second_later_land(self):
        a1, a2, m1, m2 = ("%040x" % i for i in range(1, 5))
        index = self.index(dict(id=100, t=1000, commits={self.apps: a1}),
                           dict(id=101, t=2000, commits={"makepad": m2}, pre={"makepad": m1}),
                           dict(id=102, t=2000, commits={self.apps: a2}))
        self.rev(self.apps, a2 + "^1", a1)
        expected = (("hybrid-" + m1, ((self.apps, a1),)), ("hybrid-" + m2, ((self.apps, a1),)))
        self.assertEqual(self.states(index.records[1], index), expected,
                         "record order must keep both sides before the same-second apps land")
        self.ancestor.return_value = False
        self.at(self.apps, 2000, "HEAD", a1)
        self.assertEqual(self.states(index.records[1], index), expected,
                         "an unfetched next land must bound lookup at the checkout tip")

    def test_non_devhub_commits_stay_single_repo_and_respect_land_boundaries(self):
        self.nested.append(self.library)
        a1, a2, s1, s2 = ("%040x" % i for i in range(1, 5))
        index = self.index(dict(id=100, t=1000, commits={self.apps: a1}),
                           dict(id=101, t=1501, commits={self.apps: a2}))
        items = self.touching({self.library: [(1600, s2, "second direct edit"), (1500, s1, "")]}, index)
        self.assertEqual([(l.land_id, l.commits) for l in items], [(None, ((self.library, s1),)),
                                                                  (None, ((self.library, s2),))],
                         "direct commits must remain separate and ordered, including an empty subject")
        self.at("makepad", 1500, "HEAD", "m0")
        self.rev(self.library, s1 + "^1", "")
        self.rev(self.apps, a2 + "^1", a1)
        self.at(self.apps, 1500, a1, a1)
        self.assertEqual(self.states(items[0], index), (
            ("hybrid-m0", ((self.apps, a1), (self.library, live.EMPTY))),
            ("hybrid-m0", ((self.apps, a1), (self.library, s1)))),
            "the direct root commit must start at EMPTY and never cross the next apps land")

    def test_git_fallback_groups_same_branch_within_window_and_splits_outside(self):
        self.nested.append(self.library)
        fab = self.subject("rc-fabvprint")
        rows = {"makepad": [(1000, "m1", self.subject("rc-fabvprint", "work")), (1000, "m2", self.subject("rc-other"))],
                self.apps: [(1011, "a2", fab)], self.library: [(1000, "s1", "direct edit")]}
        self.scan(1000, 1011, {self.apps: [(1001, "a1", fab), (1011, "a2", fab)]})
        items = self.touching(rows)
        pair = (("makepad", "m1"), (self.apps, "a1"))
        self.assertCountEqual([l.commits for l in items], [pair, ((self.apps, "a2"),), (("makepad", "m2"),),
                                                           ((self.library, "s1"),)],
                              "only same-branch merges within the window should pair, even outside the closure")
        self.rev("makepad", "m1^1", "m0")
        self.rev(self.apps, "a1^1", "a0")
        self.at(self.library, 1000, "HEAD", "s1")
        self.assertEqual(self.states(next(l for l in items if l.commits == pair)), (
            ("hybrid-m0", ((self.apps, "a0"), (self.library, "s1"))),
            ("hybrid-m1", ((self.apps, "a1"), (self.library, "s1")))),
            "both inferred halves must move from their own first parents")

    def test_resolve_only_groups_and_deduplicates_halves(self):
        row = self.fab_record()
        index = self.index(row)
        halves = (("makepad", self.makepad), (self.apps, self.fab))
        rows = {}
        for repo, sha in halves:
            self.rev(repo, sha[:10] + "^{commit}", sha)
            self.rev(repo, sha + "^{commit}", sha)
            rows[repo] = [(self.timestamp, sha, self.subject("rc-fabvprint", "work" if repo == "makepad" else "main"))]
            self.log(repo, rows[repo], "-1", sha)
        self.scan(self.timestamp, self.timestamp, rows)
        for records in (index, None):
            for revs in ([self.makepad[:10]], [self.fab[:10]], [self.makepad[:10], self.fab[:10]]):
                with self.subTest(indexed=records is not None, revs=revs):
                    items = live.resolve_only(revs, self.tips, records)
                    self.assertEqual([l.commits for l in items], [halves], "either or both halves must replay once")
                    self.assertEqual(items[0].land_id, 10593 if records else None, "indexed lookup must keep the land id")
                    self.assertNotIn(self.commercial, dict(items[0].pre), "an absent repo must be omitted from pre")
        self.rev(self.apps, self.fab + "^{commit}", "")
        with self.assertRaisesRegex(SystemExit, "missing commit", msg="an indexed half missing on disk must abort"):
            live.resolve_only([self.makepad[:10]], self.tips, index)

    def test_load_landings_missing_or_corrupt_fails_loudly(self):
        path = self.root / "input.jsonl"
        with self.assertRaises(SystemExit, msg="missing explicit land records must abort"):
            live.load_landings(str(path))
        for bad in ("{", "[]", '{"id":1,"t":1}', json.dumps(dict(self.fab_record(), commits={self.apps: "short"})),
                    json.dumps(dict(self.fab_record(), id=True)), json.dumps(dict(self.fab_record(), t=False)),
                    json.dumps(dict(self.fab_record(), post={self.apps: "short"}))):
            with self.subTest(bad=bad):
                path.write_text("\n" + bad)
                with self.assertRaisesRegex(SystemExit, "input.jsonl:2:", msg="invalid rows must identify the file and line"):
                    live.load_landings(str(path))
        marker = dict(window=1, older=2, newer=0, total=3)
        for malformed in (None, {"window": 1}, dict(marker, older=-1), dict(marker, total=True),
                          dict(marker, newer=0.5), dict(marker, extra=1)):
            with self.subTest(trimmed=malformed):
                path.write_text("\n" + json.dumps(dict(self.fab_record(), trimmed=malformed)))
                with self.assertRaisesRegex(SystemExit, "input.jsonl:2:.*trimmed", msg="a malformed marker must identify its line"):
                    live.load_landings(str(path))
        path.write_text(json.dumps(self.fab_record()) + "\n\n" + json.dumps(dict(self.fab_record(), trimmed=marker)))
        with self.assertRaisesRegex(SystemExit, "input.jsonl:3:.*trimmed", msg="a trim marker on the second record must be rejected"):
            live.load_landings(str(path))

    def test_chain_orders_grouped_landings_oldest_first(self):
        old = live.Landing(1000, 0, (("makepad", "m1"), (self.apps, "a1")))
        new = live.Landing(2000, 0, (("makepad", "m2"), (self.apps, "a2")))
        for repo, prefix in (("makepad", "m"), (self.apps, "a")):
            for n in (1, 2):
                self.rev(repo, prefix + str(n) + "^1", prefix + str(n - 1))
        self.answers["makepad", ("rev-parse", "HEAD")] = "HEAD"
        chain = Mock(return_value=0)
        with patch.multiple(live, WORK=str(self.root), TREE=str(self.root), heads=lambda: [(self.apps, "HEAD")],
                            metadata=lambda: {}, kept_dirs=lambda meta: [], hybrid=lambda sha, tip, kept: sha,
                            resolve_only=Mock(return_value=[new, old]), chain=chain), \
                patch.object(live.sys, "argv", ["live_replay", "--chain", "--only", "m2,m1"]), \
                patch.object(live.subprocess, "run"), patch.object(live.shutil, "copy2"), redirect_stdout(io.StringIO()), \
                self.assertRaises(SystemExit, msg="chain mode must exit with its own result"):
            live.main()
        states = [("m%d" % n, ((self.apps, "a%d" % n),)) for n in range(3)]
        self.assertEqual(chain.call_args.args[1], [(old, states[0], states[1]), (new, states[1], states[2])],
                         "chain must apply the older complete cross-repo transition first")


class FakeApp(live.App):
    def __init__(self, log="", answers=()):
        self.log = log
        self.ask = Mock(side_effect=answers)
        self.pid = 100
        self.held = self.parked = None
        self.proc = SimpleNamespace(pid=99, returncode=0, wait=lambda: 0)
        self.file = io.StringIO()

    def text(self):
        return self.log

    def alive(self):
        return True


class FakeClock:
    def __init__(self):
        self.now = 100.0
        self.sleeps = []

    def time(self):
        return self.now

    def sleep(self, dt):
        self.sleeps.append(dt)
        self.now += dt


class HeldReplayTests(unittest.TestCase):
    ended = ("relax live: the run ends in place: its app check found 4 errors: "
             "test_prints.rs:361:35: E0599: no method named 'hovered' found for 'PropSection' "
             "in the current scope; the next save or restart retries it")

    def setUp(self):
        self.clock = FakeClock()
        clock = patch.object(live, "time", self.clock)
        clock.start()
        self.addCleanup(clock.stop)
        killpg = patch.object(live.os, "killpg")
        self.killpg = killpg.start()
        self.addCleanup(killpg.stop)

    def test_wait_frame_ends_at_once_on_ended_in_place_log(self):
        app = FakeApp(self.ended)
        why = app.wait_frame(0, 1500)
        self.assertEqual(why, "its run ended in place: " + self.ended, "frame wait must report the hold line")
        self.assertEqual(app.held, self.ended, "cleanup must know the run ended")
        self.assertEqual(self.clock.sleeps, [], "the log must end the first iteration before sleep")
        self.assertEqual(app.ask.call_count, 0, "a logged hold needs no inspect call")

    def test_wait_frame_ends_on_live_held_without_log_line(self):
        app = FakeApp("[makepad-remote] pid=100 app=fab", [
            (0, '{"held":1,"restart_unfinished":0,"restart_pending":0,"pending_why":""}')])
        start = self.clock.time()
        why = app.wait_frame(1, 600, since=0, await_relaunch=True)
        self.assertEqual(why, "its host held: host held with no running app", "a held host needs no pending restart")
        self.assertEqual(app.held, "host held with no running app", "the fallback hold reason must be retained")
        self.assertEqual(app.ask.call_args_list, [call("live", timeout=10)], "the first live poll must end the wait")
        self.assertEqual(self.clock.time() - start, live.LIVE_POLL, "a frameless wait must poll at LIVE_POLL")

    def test_wait_frame_relaunch_waits_for_restart_unfinished_then_catches_late_check_failure(self):
        reason = "the last run ended in place: its app check found 1 error: a.rs:1"
        app = FakeApp("first frame at 0\nfirst frame at 1", [
            (0, '{"held":0,"restart_unfinished":1}'),
            (0, '{"held":1,"restart_unfinished":0,"pending_why":"%s"}' % reason)])
        why = app.wait_frame(1, 600, await_relaunch=True)
        self.assertEqual(why, "its host held: " + reason, "a frame before the failed check cannot complete relaunch")
        self.assertEqual(app.held, reason, "the late check must retain the host's reason")
        self.assertEqual(app.ask.call_args_list, [call("live", timeout=10)] * 2, "both app-check states must be read")
        self.assertEqual(self.clock.sleeps, [0.25], "a new frame must start the relaunch check immediately")

    def test_wait_frame_relaunch_succeeds_when_restart_unfinished_clears(self):
        app = FakeApp("first frame at 0\nfirst frame at 1", [
            (0, '{"held":0,"restart_unfinished":1}'), (0, '{"held":0,"restart_unfinished":0}')])
        self.assertIsNone(app.wait_frame(1, 600, await_relaunch=True), "a checked frame must complete relaunch")
        self.assertEqual(app.ask.call_args_list, [call("live", timeout=10)] * 2, "the unfinished frame must wait")
        self.assertIsNone(app.held, "a successful relaunch must leave the app running")

    def test_wait_frame_ends_on_postponed_and_other_parked_and_host_rebuild_failed(self):
        for line, options, prefix in (
            ("restart postponed (struct Foo changed): unsaved", {"await_relaunch": True}, "its restart was postponed: "),
            ("relax: panic parked on thread main", {}, "its JIT parked: "),
            ("relax: stale access parked as P4", {}, "its JIT parked: "),
            ("relax: update failed -- parked as P2", {}, "its JIT parked: "),
            ("relax live: rebuilding makepad FAILED", {"watch_rebuild": True}, "makepad rebuild failed: "),
        ):
            with self.subTest(line=line):
                old = self.ended + "\n"
                app = FakeApp(old + line)
                why = app.wait_frame(0, 600, since=len(old), **options)
                self.assertEqual(why, prefix + line, "only the new failure should end the frame wait")
                if "parked" in line:
                    self.assertEqual(app.parked, line, "a logged park must reach cleanup")
                self.assertEqual(self.clock.sleeps, [], "logged failures must end the wait immediately")
        with self.subTest(source="inspect session"):
            app = FakeApp(answers=[(0, '{"held":0}'), (0, '{"restart":{"phase":"Failed","failed":"cargo error"}}')])
            why = app.wait_frame(0, 600, watch_rebuild=True)
            self.assertEqual(why, "makepad rebuild failed: cargo error", "session failure must report the rebuild error")
            self.assertEqual(app.ask.call_args_list, [call("live", timeout=10), call("session", timeout=10)],
                             "host rebuild must inspect both live and session on the first poll")

    def test_stop_skips_gq_when_held_or_parked(self):
        for state, reason in (("held", "ended in place"), ("parked", "compile error parked")):
            with self.subTest(state=state):
                app = FakeApp()
                setattr(app, state, reason)
                self.killpg.reset_mock()
                self.assertEqual(app.stop(), "", "ending a known hold or park must produce no quit-hang report")
                self.assertEqual(app.ask.call_count, 0, "cleanup must skip gq and why-hung")
                self.assertEqual(self.killpg.call_args_list, [call(99, 9)], "cleanup must stop the owned process group")

    def test_verdict_ends_immediately_on_held_and_panic_event(self):
        ended = "relax live: the run ends in place: its app check found 2 errors"
        for outcome, log, answer, reason in (
            ("held", ended, (1, '{"err":"not a live session"}'), "its host held: " + ended),
            ("parked", "", (0, '{"event":"panic","kind":"compile error","message":"E0599"}'),
             "its JIT parked: compile error: E0599"),
        ):
            with self.subTest(outcome=outcome):
                app = FakeApp(log, [answer])
                self.assertEqual(live.verdict(app, 3, 0), {"outcome": outcome, "error": reason},
                                 "the first terminal state must supply the verdict and its reason")
                self.assertEqual(app.ask.call_args_list, [
                    call("watch", "on=patchverified,patchrefused,patchreverted,panic", "since=3", "timeout_s=10", timeout=60)
                ], "one watch must capture a hold or panic")
                self.assertEqual(self.clock.sleeps, [], "a known verdict must never wait for the watcher")
                self.assertEqual(app.parked if outcome == "parked" else app.held,
                                 "compile error: E0599" if outcome == "parked" else ended,
                                 "the verdict must preserve the reason for cleanup")

    def test_bad_lines_take_a_threads_panic_not_a_name_that_says_panicked(self):
        # (an O3 build's note names `ScriptVmCall::resume_if_panicked`: a 40-landing replay judged
        # the app's clean restart a panic, 11-28)
        notes = [
            "relax: the O3 build of #56563 view:: leaves inline candidate #4582 ScriptVmCall::resume_if_panicked out until the owner builds what it needs: it needs #4582 ScriptVmCall::resume_if_panicked (its front is not made off the owner: unchecked)",
            "relax: the O3 build of #95068 view:: leaves inline candidate #143328 Iterator::any out until the owner builds what it needs: it needs #143328 Iterator::any (its front is not made off the owner: panic)",
        ]
        panics = ["thread 'main' (4242) panicked at apps/fab/app/src/main.rs:3:5:", "[E] JIT call panicked: index out of bounds"]
        self.assertEqual(live.bad_lines("\n".join(notes) + "\n"), [], "a note naming a panic-handling function is no panic")
        self.assertEqual(live.bad_lines("\n".join(notes + panics) + "\n"), panics, "a thread's panic and a JIT call's are panics")

    def test_replay_once_parent_held_classifies_skip_vs_agreement(self):
        for accepts, result, kind in ((False, "skip", "tree"), (True, "fail", "agreement")):
            with self.subTest(accepts=accepts):
                rustc = "error[E0599]: no method named 'hovered'" if not accepts else ""
                with patch.object(live, "App", return_value=FakeApp(self.ended)), patch.object(live, "move_tree"), \
                        patch.object(live, "rustc_accepts", return_value=(accepts, rustc)):
                    rec = live._replay_once("8b6ce4e23b", "parent", "landing", "makepad-fab", 0)
                self.assertEqual((rec["result"], rec["kind"]), (result, kind), "rustc decides parent agreement")
                self.assertIn(self.ended, rec["why"], "the recorded reason must quote the JIT hold")
                if not accepts:
                    self.assertIn(rustc, rec["why"], "the skip must also quote rustc's diagnostic")

    def test_relaunch_held_classifies_skip_vs_fail_and_host_never_skips(self):
        for check_tree, accepts, result, kind in (
            (True, False, "skip", "tree"), (True, True, "fail", "restart"), (False, False, "fail", "restart"),
        ):
            with self.subTest(check_tree=check_tree, accepts=accepts):
                app = FakeApp(self.ended)
                with patch.object(live, "rustc_accepts", return_value=(accepts, "" if accepts else "E0599")) as rustc:
                    actual = live.relaunch(app, "makepad-fab", "the restart", check_tree=check_tree)
                self.assertEqual(actual[:2], (result, kind), "only a rejected replay tree may skip relaunch")
                self.assertIn(self.ended, actual[2], "a held relaunch must explain the hold")
                self.assertEqual(rustc.call_args_list, [call("makepad-fab")] if check_tree else [],
                                 "the host must never check the replay tree")
                self.assertEqual(app.ask.call_count, 0, "an already held run must skip session relaunch")
        with self.subTest(when="healthy after the checked frame"):
            app = FakeApp(answers=[(0, '{"held":0}'), (0, "relaunched"), (0, "idle")])

            def frame(*args, **kwargs):
                app.log += self.ended
                return None

            app.wait_frame = Mock(side_effect=frame)
            with patch.object(live, "rustc_accepts", return_value=(False, "E0599")):
                result, kind, why = live.relaunch(app, "makepad-fab", "the restart")
            self.assertEqual((result, kind), ("skip", "tree"), "a late held health check must classify the tree")
            self.assertIn(self.ended, why, "the late skip must quote the JIT hold")
            self.assertIn("E0599", why.split("; the JIT:")[0], "the late skip must quote rustc's diagnostic")
            self.assertEqual(app.wait_frame.call_args_list, [call(0, live.BOOT, 0, await_relaunch=True)],
                             "session relaunch must wait for its app check")

    def test_replay_once_parent_late_held_before_move_tree_judges_parent(self):
        app = FakeApp("first frame at 0", [(0, "idle")])

        def healthy(*args):
            app.log += "\n" + self.ended
            return None

        with patch.object(live, "App", return_value=app), patch.object(live, "RESIZE", False), \
                patch.object(live, "healthy", side_effect=healthy), patch.object(live, "move_tree") as move:
            def rustc(package):
                self.assertEqual(move.call_args_list, [call("parent")], "rustc must judge the parent still on disk")
                return False, "error[E0599]"

            with patch.object(live, "rustc_accepts", side_effect=rustc) as check:
                rec = live._replay_once("8b6ce4e23b", "parent", "landing", "makepad-fab", 0)
        self.assertEqual(check.call_args_list, [call("makepad-fab")], "the late parent hold needs a rustc verdict")
        self.assertEqual(move.call_args_list, [call("parent")], "a held parent must end replay before the edit")
        self.assertEqual(app.ask.call_args_list, [call("jit", "ms=100")], "a held parent must skip the events query")
        self.assertEqual((rec["result"], rec["kind"]), ("skip", "tree"), "a rejected late-held parent must skip")


class ReplayTests(unittest.TestCase):
    def test_clock_crossed_boundaries(self):
        # Choose the local clock independently of the machine running the suite.
        utc_hour = (utc(28, 13, 59, 55), utc(28, 14, 0, 5))
        half_hour = (utc(27, 18, 29, 55), utc(27, 18, 30, 5))
        daylight = time.struct_time((*time.gmtime(SAME_HOUR[0])[:8], 1))
        standard = time.struct_time((*time.gmtime(SAME_HOUR[1])[:8], 0))
        for name, times, local_times, crossed in (
            ("UTC midnight", (utc(27, 23, 59, 55), utc(28, 0, 0, 5)), None, True),
            ("same hour", (utc(28, 0, 0, 5), utc(28, 0, 0, 55)), None, False),
            ("UTC hour with unchanged local hour", utc_hour,
             [time.gmtime(t + 5.5 * 3600) for t in utc_hour], True),
            ("local midnight at UTC+05:30", half_hour,
             [time.gmtime(t + 5.5 * 3600) for t in half_hour], True),
            ("DST fallback in repeated hour", SAME_HOUR, [daylight, standard], True),
        ):
            with self.subTest(name=name), patch.object(live.time, "localtime",
                                                       side_effect=local_times or time.gmtime):
                self.assertEqual(live.clock_crossed(*times), crossed)

    def test_layout_diff_storybook_day_rollover(self):
        today = days(84)
        noise = live.differing(today, today)
        self.assertIn("only in one", live.layout_diff(days(83), today, noise, "new process"))
        self.assertIsNone(live.layout_diff(today, today, noise, "new process"))

    def replay_records(self, times, *records):
        args = ("landing", "parent tree", "edited tree", "storybook", 1)
        with patch.object(live, "_replay_once", side_effect=records) as attempt, \
                patch.object(live.time, "time", side_effect=times):
            result = live.replay(*args)
        self.assertEqual(attempt.call_args_list, [call(*args)] * len(records))
        return result

    def test_replay_stale_restart_across_midnight_retries_and_passes(self):
        first = record("fail", "stale_restart", why="83 days vs 84 days", quit="gq: stuck")
        first["s"]["total"] = 45.7
        result = self.replay_records(MIDNIGHT, first, record("unreached"))
        self.assertEqual((result["result"], result["kind"], result["rung"]), ("unreached", "", "none"))
        self.assertEqual(result["clock_retry"],
                         "stale_restart across 2026-11-27T23:59:18Z -> 2026-11-28T00:00:04Z: 83 days vs 84 days")
        self.assertEqual(result["s"]["attempt1"], 45.7)
        self.assertEqual(result["quit"], "gq: stuck")

    def test_replay_stale_layout_across_midnight_retries_and_passes(self):
        result = self.replay_records(MIDNIGHT, record("fail", "stale_layout"), record("pass", rung="applied"))
        self.assertEqual((result["result"], result["rung"]), ("pass", "applied"))
        self.assertTrue(result["clock_retry"].startswith("stale_layout across "))

    def test_replay_layout_fail_same_hour_does_not_retry(self):
        for kind in ("stale_restart", "stale_layout"):
            with self.subTest(kind=kind):
                result = self.replay_records(SAME_HOUR, record("fail", kind))
                self.assertEqual((result["result"], result["kind"]), ("fail", kind))
                self.assertNotIn("clock_retry", result)

    def test_replay_real_stale_restart_across_midnight_fails_after_one_retry(self):
        result = self.replay_records(MIDNIGHT, record("fail", "stale_restart", why="first diff"),
                                     record("fail", "stale_restart", why="second diff"))
        self.assertEqual((result["result"], result["kind"], result["why"]), ("fail", "stale_restart", "second diff"))
        self.assertIn("first diff", result["clock_retry"])

    def test_replay_non_layout_failures_never_retry_across_midnight(self):
        kinds = ("panic", "ice", "crash", "draw_error", "error_view", "parked", "dead_ui", "restart", "agreement")
        for first in [record("fail", kind) for kind in kinds] + [record("silent"), record("pass")]:
            with self.subTest(result=first["result"], kind=first["kind"]):
                result = self.replay_records(MIDNIGHT, first)
                self.assertIs(result, first)
                self.assertNotIn("clock_retry", result)


class HostTests(unittest.TestCase):
    def run_host(self, root, times, restarted_days, boot_error=None):
        source = Path(root, "host.rs")
        source.write_text("original")
        observed = []
        snapshots = []
        for count in restarted_days:
            snapshots.extend([days(count), days(count), days(count), days(84)])

        def edit(*args):
            observed.append(source.read_text())
            source.write_text("edited")
            return dict(record("pass"), what="host edit")

        boots = iter([state for _ in restarted_days for state in ("original", "edited")])

        def start_app(*args, **kwargs):
            expected = next(boots)
            self.assertEqual(source.read_text(), expected, "host source at process startup")
            app = Mock()
            app.text.return_value = ""
            app.wait_frame.return_value = boot_error if expected == "edited" else None
            return app

        output = io.StringIO()
        with ExitStack() as scope:
            scope.enter_context(redirect_stdout(output))
            scope.enter_context(patch.multiple(
                live, ROOT=root, WORK=root, HOST_EDITS=[("edit", "host.rs")],
                App=start_app, host_case=edit, relaunch=Mock(return_value=None),
                settle=Mock(), healthy=Mock(return_value=None), layout=Mock(side_effect=snapshots),
            ))
            scope.enter_context(patch.object(live.time, "time", side_effect=times))
            scope.enter_context(patch.object(live.time, "sleep", side_effect=AssertionError("unexpected wait")))
            code = live.host("storybook", False)
        self.assertEqual(observed, ["original"] * len(restarted_days), "each attempt must start from the original file")
        self.assertEqual(source.read_text(), "original", "the last attempt must restore its edit")
        return code, output.getvalue()

    def test_host_stale_restart_across_midnight_retries_and_restores_files(self):
        with TemporaryDirectory() as root:
            code, output = self.run_host(root, MIDNIGHT + SAME_HOUR, (83, 84))
        self.assertEqual(code, 0)
        self.assertIn("retrying once: stale_restart across ", output)

    def test_host_real_failures_and_boot_failure(self):
        with TemporaryDirectory() as root:
            for name, times, counts, boot_error, kind in (
                ("same hour", SAME_HOUR, (83,), None, "stale_restart"),
                ("persistent across midnight", MIDNIGHT + SAME_HOUR, (83, 83), None, "stale_restart"),
                ("fresh boot failure", MIDNIGHT, (83,), "no first frame", "restart"),
            ):
                with self.subTest(name=name):
                    code, output = self.run_host(root, times, counts, boot_error)
                    self.assertEqual(code, 3)
                    self.assertRegex(output, r"new process\s+fail\s+-\s+" + kind + r"\s")
                    if boot_error:
                        self.assertIn(boot_error, output)

    def test_wm_session_clears_stale_socket(self):
        with TemporaryDirectory() as work:
            run = Path(work, "wm-run")
            run.mkdir()
            old = run / "makepad-wm-old"
            new = run / "makepad-wm-new"
            old.touch()
            app = Mock()

            def start_wm(*args, **kwargs):
                self.assertFalse(old.exists(), "the previous socket must be gone before WM startup")
                return app

            app.wait_frame.side_effect = lambda *args: new.touch()
            with patch.object(live, "WORK", work), patch.object(live, "App", start_wm), \
                    patch.object(live.time, "sleep", side_effect=AssertionError("unexpected wait")):
                session, socket = live.wm_session()
            self.assertIs(session, app)
            self.assertEqual(socket, str(new))


if __name__ == "__main__":
    unittest.main()
