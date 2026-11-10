# Dev workflow: from an agent's edit to a verdict

How lanes build, test, land and run apps through devhub and the JIT, and where the time
goes. `devhub <cmd> --help` is the command reference; this page is the why and the order.

## 0. The loop at a glance

An agent edits in its lane worktree. A live session ships each saved file to a box. There
the JIT patches it into the running app or the warm test process and answers with a
one-line verdict. The lane hands in with `devhub land`, which only merges. Buckets check the
landed tips afterwards, every hub moves to them, and running apps patch forward under the
JIT. The numbers are one day of all lanes (medians / p90):

| Step | Tool | Typical latency |
|---|---|---|
| save → shipped to the box | `devhub live` sync | 0.1 s / 0.4 s |
| shipped → patched | the box's JIT | 3.6 s / 86 s (p90: a cold JIT or cache) |
| patched → tests rerun or frame drawn | the warm process | 4.4 s / 63 s |
| save → verdict, end to end | `live wait <id> verdict`, `live look <id>` | 15 s / 138 s |
| one JIT run as a job (queue included) | `devhub job submit` / `job wait` | 112 s / 489 s |
| hand-in → landed | `devhub land` | seconds behind the queue ahead |
| landed → fast bucket verdict | checks and presents of what it reached | about 3 min a run |
| landed → apps booted | hot bucket | about 1 min |
| landed → every hub runs it | self-update | 5.8 min / 10.3 min |
| landed → affected tests | tests bucket | 23-48 min a run |
| landed → origin | `devhub push`, reviewed | once a day |

## 1. The pieces

- **The master**: one devhub process (`devhub serve`) beside the user's checkout.
  It holds the queue, the landing train, the buckets, the merge bot and the ledgers. It
  dials the hubs and listens on nothing but its control socket.
- **A hub per box**: serves that box's JITs, runs its leases, reports its capabilities and
  pressure, and keeps one JIT build and one code cache for every lane on the box.
- **Box roles** (`devhub node <box> role ...`):
  - pool: builds, tests, screens, live sessions;
  - hot: small jobs on a warm JIT with a fast handoff;
  - bench: timed work. It refuses work any pool box can take;
  - the user's app box: a person's display. Lanes show apps there and never take it;
  - device: phones and headsets, through their own commands.
- **Self-update**:
  - the master builds work's tip and execs into it in place: at most every 10 min, within
    5 min of a landing; leases, queued requests and launches survive it;
  - each hub rolls to the landed tip (`devhub rolls`), a median 5.8 min after a landing
    (p90 10.3);
  - after a landing each box prebuilds its JIT and warms the smoke apps' caches
    (`devhub prewarm`); a lease preempts it.

## 2. Lanes

- One lane per task: `devhub lane new <name>` makes worktrees in both repos (makepad on
  `rc-<name>` from work, apps/commercial on `rc-<name>` from main) and the devhub record.
  `--no-commercial` for a makepad-only lane. Never `git worktree add`; `devhub lane adopt`
  registers one that exists.
- Lane notes and scratch state go in `local/agent_state/<lane>/`, never in tracked files.
- The merge bot keeps every lane merged with the tip, in the lane's own worktree. It waits
  while a job, a live session or a recent timed lease of the lane runs there.
  - `devhub hold <branch> "why"` keeps it off a lane whose pinned binary you compare against;
    `devhub unhold <branch>` after the last run.
  - Its merges land on your branch in your worktree: check that HEAD is yours before an
    amend.
- When a lane is done:
  - `devhub lane park <name> [why]`: it waits (finished, or for another lane); the merge bot
    leaves it alone. `lane unpark` to go on;
  - a landed lane is removed by the master after 30 min idle (`lane keep` exempts it);
  - `lane retire <name>`: over without landing (branches kept);
  - `lane drop <name>`: abandon it (unlanded commits are bundled first).
- `devhub lanes` and `devhub lane quiet` list lanes, their merge and land state, and which
  ones went quiet.

## 3. Leases and the queue

Everything that runs on a box runs under a lease: `run`, `test`, `job`, `live` and the
buckets ask for one, and the queue decides where and when.

- **Kinds**:
  - build and test: the bulk;
  - screen: opens windows on a box that presents, hidden from the person
    (`MAKEPAD_HIDE_WINDOWS=1`, as in every lease). A person's launch is
    `devhub show <app> --on <node>`, with `--native` for the compiled app: the
    node's WM opens it, with no lease and no test instance, and its tile shows the build
    (`devhub show stop <app> --on <node>` closes it: left open, it holds that box's WM restart);
  - timed: the box to itself, for timings;
  - gate: a landing gate;
  - gpu, long, inspect and ci;
  - a live session holds a small lease on a hot box when one is free.
- **Cost**: `--cost small|medium|large` or `--est-min N`. Small work goes to the hot boxes.
  Estimates come from the run class's measured times; devhub corrects a bad guess.
- **Run classes**: devhub measures each class's time, cores and peak RAM. A JIT class names
  what runs and lanes share it; any other command is measured per lane. `--class <name>`
  names one. Estimates, stall warnings and RAM asks read the class.
- **Priority**: interactive (an agent or a person waits on it) or background (buckets after
  their gate, prebuilds, CI, correctness legs). On a shared box interactive work gets a
  hundred times the CPU share of background work.
- **Budgets**: each box has a cores budget and a RAM budget.
  - A lease is charged its asked cores (`--cores`), or its class's share. An ask under its
    class's measured cores is charged the measure, and its run still gets only what it asked.
  - RAM is what it asked (`--ram-gb`) or its class's measured peak, cut to the box's
    budget.
  - On Linux, a live launch or call doesn't count background shared leases' cores, or the
    box's measured CPU: it runs ahead of them at the higher CPU weight, and no room is kept
    for calls. Its lease line says when that takes the box past its budget, and by how much.
    RAM, GPUs, timed work, gates, owed legs and other interactive work still count. Other
    systems keep room for one call of their largest idle session.
- **Queue rules**:
  - Landing gates go first. A gate that waited a minute takes the cores its box has free
    (at least 4). From then on it drains the box: no newer work starts there unless it ends
    first or fits beside it.
  - A request waiting for a whole box, a tests shard or a landing gate holds its box. Only
    work that ends before it, or fits beside it, starts there. Interactive work may pass a
    background whole-box hold.
  - A post-land tests shard asks the RAM of its largest unit's last peak
    (`--ram-floor-gb`). A shard that waits is resized to fit, never below that floor.
  - A request names `--node` only when it must; a pinned request waits for that one box.
  - `lane first <name> on` (the user's call) puts a lane's leases ahead of every other
    lane's and the land queue's.
- **Reading a wait reason** (`devhub queue`, `devhub why-queued <lease>`), for example:
  `L<lease> test: 2nd in queue (<box>: budget cores <n>, in use <n>, asked <n> (+<k> nodes))`
  - budget cores / budget ram: the box's budget is spent. It starts when something ends.
    Ask fewer cores or a smaller cost if the job allows it;
  - node busy: the box measured busy past what devhub charged (work outside leases);
  - held for landing gate / tests shard / whole box: see the rules above. It ends when the
    holder starts;
  - a request that waits long shows in the health readout. Main can `job bump` it.
  - Never cancel and resubmit: the request loses its place.

## 4. Jobs

- `devhub job submit [--kind K] [--cost C] [--lane L] -- <cmd>` returns an id at once. Then:
  - `devhub job wait <id>`, run in a background shell, wakes you when the job ends;
  - `devhub job result <id> [--tail]`: exit, error lines and the log's tail;
  - the whole log is `~/.makepad/devhub/jobs/<id>.log`.
- A plan of several jobs is one `job submit --batch <file>`, with `--after` for order.
- Outputs (sheets, grabs, logs): the command writes them to `$DEVHUB_KEEP`, and `--keep <dir>`
  copies them back before the lease ends. The lane's tree on the box may go to another lane
  right after.
- `job rerun <id>` runs the same snapshot and command again; `job cancel <id>` stops one.
- Never poll: no `sleep` or `until` loops, no repeated `job list`, `tail` or `lanes`.

## 5. JIT first

- **Apps**:
  - `devhub live start <app> [--visible] [--env K=V]` returns `W<n>` at once; every file
    saved in the worktree ships to the box and hot-patches in;
  - `devhub live look <id>`: patched, a frame drawn, and a grab in
    `local/screencap/<lane>/` (a look, not a playtest);
  - `live wait <id> patched|verdict|settled|frame` waits for one of those;
    `live act <id> <inspect cmd>` drives the app. A call goes on through a restart of the master or its
    box's hub when asking again does nothing twice (it reads, or only waited for room); any other call that is
    cut says so and isn't sent twice, and the session still runs;
  - `live release <id>` ends it. Idle sessions end on their own (apps 10 min, tests 30 min);
    `live again <id>` restarts one as it was.
- **Tests**: `devhub test <lane> <pkg> [--lib|--test T] --live [-- filter]` keeps one warm
  process per lane, package, target and args.
  - Only the affected tests rerun.
  - The verdict says save → shipped → patched → tests.
  - The same command without `--live` is the lane's pre-land check: one JIT run.
- **restart_waiting**: some edits cannot patch a running process: a type's layout or the
  dependency graph. The warm test process restarts itself. A live app keeps the old code, its
  status says `restart_waiting` and the verdict says "restart the app". Relaunch it with
  `live act <id> session relaunch`.
- **Native builds**: devhub refuses a lane's native cargo build, run or test. It allows one
  only for:
  - the JIT's own crates;
  - an open JIT miss (`devhub jit misses`);
  - a platform no JIT runs on;
  - a test that devhub's measured routing sends to cargo (it says both walls);
  - `--native "<reason>"`: native-vs-JIT baselines and timed perf series.
  The refusal names the JIT command.
- **One JIT and one cache per box**, shared by every lane:
  - code is cached by the hashes of its inputs, so a warm box rebuilds nothing that did not
    change. About four in five store lookups hit;
  - `devhub cache` and `devhub warm` show what is there;
  - a `JIT error, not your change` answer (exit 3) is not your bug: carry on.

## 6. Landing

- `devhub land` from the lane worktree pins each repository's commit and queues the
  hand-in. The train merges exactly those commits onto the tips, in queue order, and
  fast-forwards the user's checkout.
  - Check on it with `devhub land status <branch>`, or `devhub land wait <branch>` in a
    background shell.
  - A conflict or a failure is final: fix it, then hand in again (`land retry` resends the
    same pins).
- **Land = merge only.** Nothing builds or tests before the merge; checks run after it in
  buckets, and none of them blocks the train.
  - fast: right after each landing, pipelined: the checks and presents of the packages the
    landings reached;
  - hot: boots the apps a landing touched on a warm JIT, right after it;
  - fast and hot run as landing gates, first in the queue (§3);
  - tests: the affected packages' tests after a quiet minute (at most 10 min), sharded over
    the pool;
  - ci: nightly. full: every package's tests, no proofs reused, daily before the push
    review.
- **Reds**:
  - a bucket's red names the range of landings since its last green look at that unit, and
    the lanes whose inputs changed;
  - a unit failing the same way as last time is a break already on the branch: it stays with
    whoever broke it then;
  - a red stays until a run sees it pass. `devhub bucket` lists the runs.
- **Fix forward**: the lane that broke it lands a fix; landings keep coming meanwhile.
- **Pushing is a separate step**: `devhub push` sends the reviewed landed tip to origin as
  one commit per area, once a day, after the full bucket. Lanes never push.
- Train controls (main's and the user's): `land hold <branch>`, `land pause`, and
  `lane live <name> on`, which gives a landing the user waits for the fast path.

## 7. Running apps under the JIT

- Apps run from source under the JIT:
  `cargo run --profile host -p makepad-run --bin makepad -- run <package>`. The dock
  launchers do the same.
- Landed code moves forward under running apps. After a landing the box's checkout syncs
  and running apps patch to it live, keeping their state.
- A layout-changing patch (a struct's or enum's layout, the dependency graph) does not
  apply to a running app. The app keeps the old code and says `restart_waiting` until it is
  relaunched. Never restart a person's app yourself: it may hold unsaved work. Say a
  relaunch is due and let them choose.
- **The freeze rule**:
  - a patch must never swap in new code while a thread is still running code built for the
    old layout; such a patch waits for a relaunch;
  - an app that stops drawing right after a patch is a JIT fault, not the app's: read
    `why-hung`, report it with that output, and don't retry the patch.
- **Reads that are safe on someone's running app**:
  - `devhub inspect <box>/<app> --read-only <cmd>` with status, why-hung, panics, events,
    or session;
  - never `makepad inspect <app> ui` on an app your lease did not start: it walks the whole
    tree on the UI thread and can freeze the app.

## 8. Health

- Everything is under `~/.makepad/devhub/`:
  - `master.log`;
  - the ledgers `leases.jsonl`, `waits.jsonl`, `buckets.jsonl`, `rolls.jsonl` and
    `inbox.jsonl`;
  - job logs in `jobs/<id>.log`, bucket logs in `bucket-logs/<bucket>-<ms>.log`, landing logs
    in `land-logs/`, live session logs in `live-logs/`.
- Live views:
  - `devhub stream`: one line per event;
  - `devhub inbox`: what needs a look;
  - `devhub watch <kinds> --once`: block on one event;
  - the devhub panel (`makepad run makepad-app-devhub-panel`): every box's leases and load
    on one time axis, the queue, the needs and the landing strip.
- The health readout main's check-in reads, section by section:
  - machines and nodes: up or down, role, hub version, and the median handoff, setup, job
    and teardown times. A slow teardown points at a box's cleanup;
  - leases: running and queued now, and the timed queue per box;
  - queued over 10 min: each long wait with its reason (§3);
  - queue waits, last hour: the agents' wait median, p90 and max (`devhub stats queue
    --hour`);
  - top blockers: the wait reasons ranked by minutes (`devhub prof`);
  - lease expiries and errors, last 20 min: a holder that stopped beating, a box that lost
    one;
  - jit first, last hour: per lane, native builds against saves patched live. OVER marks a
    lane that builds natively more than it patches;
  - post-land buckets: the last runs, reds and who they name (`devhub bucket`);
  - lanes quiet: lanes with nothing running for 15 min (`devhub lane quiet`);
  - tool calls in flight over 15 min: an agent stuck on one call;
  - tokens and work: what each lane spent and landed.

## 9. Latency: from an agent's edit to a verdict

One day of every lane, medians / p90s. Where we are still slow, it says so.

- **Save → shipped → patched → verdict → frame**: 0.1 / 0.4 s to ship, 3.6 / 86 s to patch,
  4.4 / 63 s to run, 15 / 138 s end to end. The p90 is a cold JIT, a cold cache or a slow
  test, not the wire.
- **Warm against cold**:
  - a warm patch takes seconds; a cold one is a minute or more;
  - caches are keyed by input hashes, so warm means "the inputs did not change", not
    "recently used";
  - prebuilding after each landing keeps every box warm for the next lease;
  - one JIT per box means a lane never pays for another lane's build twice.
- **Hot boxes for small jobs**: handoff, setup and teardown take 0.1-0.2 s (median). A small
  job's whole run is about 30-36 s.
- **JIT against native**: the devhub library's tests take about 40 s as a JIT run against
  about 2.3 min native. Prefer `test --live` while iterating; only affected tests rerun.
- **Affected-only test buckets**: 23-48 min a run, against 78-107 min when every unit ran.
- **Landing gates first**: the fast gate's queue wait was 37 s median and 4 min p90 before
  gates went first; 20-108 s since.
- **Why we never poll**:
  - every tool round re-reads the agent's whole context, so a poll costs a full round each
    time;
  - a background `job wait`, `land wait` or `live wait` wakes the agent once, when the
    answer is there;
  - waiting and checking fell from 17% of agent cost to 10%; repeat checks are 2%.
- **Token-lean, AI-first answers**:
  - `live look`, `live wait … verdict`, `job result`, `why-hung` and `land status` each give
    their verdict in one line, with details only on request (`--tail`);
  - batch reads you can already name into one call.
- **Still slow**:
  - agent wait: 53 s median, but 6.0 min p90 and 65 min max in a day. Waiting for cores is
    the top blocker, at over a third of all queued minutes;
  - cold patches: 86 s p90;
  - one-off jobs: 489 s p90, mostly queue;
  - the tests bucket: 23-48 min, so a red can surface half an hour after its landing;
  - hubs run a landing 6 min after it (p90 10 min), so a fix reaches every box that much
    later.

## 10. How the agents work

- **Main** coordinates and decides:
  - it writes each lane's brief, reviews diffs, and relays the user's calls;
  - it owns the land train's controls;
  - its standing check-in reads the health readout (§8) and fixes hiccups at once. It sends
    root causes to a devhub lane, not round them.
- **Lane agents** each own one devhub lane and one brief:
  - they never start other agents;
  - they report to main briefly at each landing, with the sha and the measured effect;
  - they keep their state in `local/agent_state/<lane>/`, and write a resume note there
    before their context runs out.
- **Decide rather than wait**: a lane that can answer its own question from the code, the
  ledgers or a run does so, and says what it chose.
- **Land = merge, fix forward**; pushing is a separate reviewed step, once a day.
