//! Deterministic, synthetic agent-work replay for comparing Studio layouts.
//! Nothing here starts a process, reads a project, or claims actual agent work.

use std::fmt::Write;

pub const DURATION: f64 = 180.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    #[default]
    AgentLanes,
    Timeline,
    SystemLanes,
}
impl Layout {
    pub fn as_str(self) -> &'static str {
        match self { Self::AgentLanes => "agent_lanes", Self::Timeline => "timeline", Self::SystemLanes => "system_lanes" }
    }
    pub fn label(self) -> &'static str {
        match self { Self::AgentLanes => "Agent lanes", Self::Timeline => "Timeline", Self::SystemLanes => "System lanes" }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentState { Waiting, Working, Testing, Blocked, Done }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind { Spawn, Think, Edit, Run, Test, Issue, Finish }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppStage { Starting, Loading, Broken, Fixed, Verified }

#[derive(Clone, Debug, PartialEq)]
pub struct AgentSnapshot {
    pub id: &'static str,
    pub parent: Option<&'static str>,
    pub name: &'static str,
    pub role: &'static str,
    pub system: &'static str,
    pub state: AgentState,
    pub task: &'static str,
    pub terminal: Vec<&'static str>,
    pub file: Option<&'static str>,
    pub diff: Vec<&'static str>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DemoEvent {
    pub id: &'static str,
    pub at: f64,
    pub agent: &'static str,
    pub kind: EventKind,
    pub title: &'static str,
    pub detail: &'static str,
    pub path: Option<&'static str>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AppSnapshot {
    pub id: &'static str,
    pub owner: &'static str,
    pub title: &'static str,
    pub route: &'static str,
    pub stage: AppStage,
    pub caption: &'static str,
    pub frame: u32,
}
#[derive(Clone, Debug, PartialEq)]
pub struct DemoSnapshot {
    pub at: f64,
    pub agents: Vec<AgentSnapshot>,
    pub events: Vec<DemoEvent>,
    pub apps: Vec<AppSnapshot>,
}

const CHECKOUT: &str = "apps/storefront/src/checkout/CheckoutSummary.tsx";
const QUOTE: &str = "services/orders/src/quote.ts";
const PAYMENT: &str = "services/orders/src/payment.ts";
const DIALOG: &str = "apps/storefront/src/checkout/AddressDialog.tsx";
const E2E: &str = "apps/storefront/tests/checkout.spec.ts";
const API_TEST: &str = "services/orders/tests/payment.test.ts";
const ORDER: [&str; 5] = ["astra", "fable", "a11y", "api", "qa"];

const fn event(id: &'static str, at: f64, agent: &'static str, kind: EventKind,
    title: &'static str, detail: &'static str, path: Option<&'static str>) -> DemoEvent {
    DemoEvent { id, at, agent, kind, title, detail, path }
}

const EVENTS: &[DemoEvent] = &[
    event("e00", 0.0, "astra", EventKind::Spawn, "Recover checkout after address changes", "Orbit Shop support reports totals jumping after shipping edits and occasional duplicate payment attempts. Establish a reproducible case before changing code.", None),
    event("e01", 4.0, "astra", EventKind::Think, "Map checkout ownership", "CheckoutSummary owns the displayed quote; the orders API owns quote revisions and payment idempotency. Delegate UI, API and independent verification.", None),
    event("e02", 9.0, "fable", EventKind::Spawn, "Trace the checkout UI", "Astra delegates the storefront: reproduce the shipping-address race and inspect the checkout interaction.", Some(CHECKOUT)),
    event("e03", 14.0, "api", EventKind::Spawn, "Inspect quote and payment contracts", "Astra delegates the orders service. Check quote revision handling and whether repeated payment submissions share an idempotency key.", Some(QUOTE)),
    event("e04", 20.0, "fable", EventKind::Run, "Start the checkout preview", "The synthetic storefront dev server is starting with a seeded basket containing one Orbit lamp.", None),
    event("e05", 24.0, "qa", EventKind::Spawn, "Prepare independent regression coverage", "Astra delegates QA. Queue a browser test that rapidly switches two shipping addresses while quote requests are in flight.", Some(E2E)),
    event("e06", 28.0, "api", EventKind::Run, "Start the order inspector", "The synthetic admin preview connects to the seeded orders service so payment attempts can be inspected alongside checkout.", None),
    event("e07", 34.0, "fable", EventKind::Run, "Load the seeded basket", "Checkout is waiting on a shipping quote. The preview shows the address form and loading total.", None),
    event("e08", 39.0, "a11y", EventKind::Spawn, "Inspect keyboard recovery", "Fable delegates a nested accessibility task: follow focus through the shipping-address dialog and return to the Pay button.", Some(DIALOG)),
    event("e09", 44.0, "qa", EventKind::Test, "FAIL: late quote replaces the current total", "The browser regression reproduces a stale response: the selected address is Amsterdam, but a delayed Utrecht quote changes the total from €84.00 to €91.00. One of six browser checks fails.", Some(E2E)),
    event("e10", 49.0, "fable", EventKind::Issue, "Stale total is visible in checkout", "Confirmed in the preview: address and total disagree after two quick edits. The Pay button remains enabled during the mismatch.", Some(CHECKOUT)),
    event("e11", 55.0, "api", EventKind::Think, "Reproduce repeated payment attempts", "The order inspector shows two pending attempts for the same basket revision. Quote caching omits the revision and payment creation does not atomically reuse a pending attempt.", Some(PAYMENT)),
    event("e12", 61.0, "fable", EventKind::Edit, "Ignore stale quote responses", "Keep a request sequence in CheckoutSummary and apply a response only when it belongs to the latest address request.", Some(CHECKOUT)),
    event("e13", 67.0, "api", EventKind::Edit, "Include basket revision in quote keys", "Quote cache keys now include basket.revision, preventing two revisions of one basket from sharing a cached shipping quote.", Some(QUOTE)),
    event("e14", 73.0, "qa", EventKind::Test, "PASS: eight quote-contract checks", "The quote revision tests pass. Browser recovery and duplicate-payment checks are still outstanding; this is not session completion.", Some(API_TEST)),
    event("e15", 79.0, "a11y", EventKind::Issue, "Focus escapes the address dialog", "Closing the dialog leaves keyboard focus on the document body. The next Tab skips the shipping control and reaches Pay unexpectedly.", Some(DIALOG)),
    event("e16", 85.0, "a11y", EventKind::Edit, "Restore focus to the shipping control", "Record the dialog trigger and restore its focus on close, including Escape and successful address submission.", Some(DIALOG)),
    event("e17", 91.0, "fable", EventKind::Run, "Preview keeps the latest address total", "Hot reload shows Amsterdam and €84.00 together after rapid address changes. A second browser pass is needed to check submission while the quote is pending.", None),
    event("e18", 98.0, "qa", EventKind::Test, "FAIL: payment can submit before quote settles", "The stale response regression now passes, but a second check catches Pay becoming enabled before the current quote resolves. Checkout briefly submits the previous basket revision.", Some(E2E)),
    event("e19", 104.0, "astra", EventKind::Issue, "Hold completion on pending-quote race", "Keep the session open. Fable must gate payment on a settled quote; the API must make payment creation atomic. QA will rerun the combined path.", None),
    event("e20", 110.0, "fable", EventKind::Edit, "Gate payment on the current quote", "Disable Pay while a quote is pending and require its basket revision to match the current basket. Show Updating total… during the transition.", Some(CHECKOUT)),
    event("e21", 116.0, "api", EventKind::Edit, "Reuse payment attempts atomically", "Create or return the attempt inside a transaction keyed by basket ID and revision. Concurrent submissions now address the same pending attempt.", Some(PAYMENT)),
    event("e22", 122.0, "api", EventKind::Run, "Restart the API and inspect one attempt", "The order inspector now shows one pending payment attempt after a repeated submission. Full browser and service regression runs are still pending.", None),
    event("e23", 128.0, "a11y", EventKind::Test, "PASS: keyboard focus returns to shipping", "The keyboard check confirms Escape and submit restore focus. The disabled Pay control is skipped while the total is updating.", Some(DIALOG)),
    event("e24", 134.0, "fable", EventKind::Run, "Replay checkout with the final fix", "The preview shows a disabled Pay button while quoting, then the current €84.00 total. One submission reaches an order confirmation.", None),
    event("e25", 140.0, "qa", EventKind::Test, "PASS: all six browser checks", "Rapid address changes, pending quote submission, retry, order confirmation and keyboard navigation pass in the seeded browser run. Final API and combined smoke checks remain.", Some(E2E)),
    event("e26", 146.0, "a11y", EventKind::Finish, "Accessibility work ready for review", "Fable receives the focus-restoration change and its keyboard evidence. The parent checkout task remains open for final integration.", Some(DIALOG)),
    event("e27", 152.0, "api", EventKind::Test, "PASS: all twelve orders-service checks", "Quote revision and concurrent payment-attempt tests pass, including two simultaneous submissions returning the same attempt ID.", Some(API_TEST)),
    event("e28", 158.0, "api", EventKind::Finish, "Orders API work ready for review", "The quote key and atomic payment changes are complete with twelve passing service checks. No deployment or merge is part of this sample.", None),
    event("e29", 164.0, "fable", EventKind::Finish, "Checkout UI work ready for review", "The stale-response guard, pending-state payment gate and nested accessibility fix are integrated. QA owns the remaining end-to-end smoke check.", Some(CHECKOUT)),
    event("e30", 171.0, "qa", EventKind::Test, "PASS: checkout and admin agree", "The final smoke run records one order and one payment attempt at €84.00. Six browser checks, twelve service checks and the keyboard check are verified in this synthetic session.", Some(E2E)),
    event("e31", 176.0, "qa", EventKind::Finish, "Verification evidence collected", "Attach the passing traces and the original failure reproduction to Astra's handoff. Checkout and the order inspector show the same settled order.", None),
    event("e32", 180.0, "astra", EventKind::Finish, "Checkout recovery ready for review", "All delegated work and regression evidence are complete. Summarize the UI race, revision-aware quotes and atomic payment reuse for human review; nothing has been merged or deployed.", None),
];

/// The complete, explicitly synthetic timeline, for drawing the replay ruler.
pub fn events() -> Vec<DemoEvent> { EVENTS.to_vec() }

fn agent(id: &'static str) -> AgentSnapshot {
    let (parent, name, role, system) = match id {
        "astra" => (None, "Astra", "Session lead", "Coordination"),
        "fable" => (Some("astra"), "Fable", "Frontend agent", "Checkout"),
        "api" => (Some("astra"), "API", "Orders service agent", "API"),
        "qa" => (Some("astra"), "QA", "Independent verification", "Quality"),
        "a11y" => (Some("fable"), "A11y", "Keyboard accessibility", "Checkout"),
        _ => unreachable!("only fixture agent IDs are replayed"),
    };
    AgentSnapshot { id, parent, name, role, system, state: AgentState::Working,
        task: "", terminal: Vec::new(), file: None, diff: Vec::new() }
}

fn terminal(id: &str) -> &'static [&'static str] {
    match id {
        "e00" => &["[session] Orbit Shop · checkout recovery", "[goal] Reproduce, fix, then verify with independent QA"],
        "e01" => &["[plan] Checkout → quote revision → payment attempt"],
        "e02" => &["$ rg 'quote|Pay' apps/storefront/src/checkout", "CheckoutSummary.tsx: quote state and payment submit"],
        "e03" => &["$ rg 'quoteKey|createAttempt' services/orders/src", "quote.ts · payment.ts"],
        "e04" => &["$ pnpm --filter storefront dev", "Starting storefront preview…"],
        "e05" => &["[queue] Reproduce two address edits with delayed quote responses"],
        "e06" => &["$ pnpm --filter orders dev", "Order inspector starting with seeded basket orbit-42"],
        "e07" => &["GET /checkout → 200", "POST /quote revision=7 → pending"],
        "e08" => &["[delegate from Fable] Inspect dialog → shipping → Pay focus path"],
        "e09" => &["$ pnpm exec playwright test checkout.spec.ts", "FAIL latest address total: expected €84.00, received €91.00"],
        "e10" => &["[reproduced] Utrecht response arrives after Amsterdam response"],
        "e11" => &["[trace] basket=orbit-42 revision=7 attempts=2", "[finding] revision missing from quote key; attempt creation is not atomic"],
        "e12" => &["[edit] CheckoutSummary.tsx: reject superseded quote requests"],
        "e13" => &["[edit] quote.ts: cache key includes basket revision"],
        "e14" => &["$ pnpm --filter orders test quote", "PASS 8 quote-contract checks"],
        "e15" => &["[keyboard] Dialog closed; activeElement=document.body", "FAIL return focus to shipping trigger"],
        "e16" => &["[edit] AddressDialog.tsx: restore trigger focus on close"],
        "e17" => &["[hmr] CheckoutSummary.tsx updated", "[preview] Amsterdam · €84.00"],
        "e18" => &["$ pnpm exec playwright test checkout.spec.ts", "FAIL pending quote: Pay enabled before revision=8 settles"],
        "e19" => &["[hold] UI quote gate + API atomic attempt required before handoff"],
        "e20" => &["[edit] CheckoutSummary.tsx: quotePending || revision mismatch", "[preview] Updating total… · Pay disabled"],
        "e21" => &["[edit] payment.ts: transaction + basket/revision idempotency key"],
        "e22" => &["[restart] Orders service ready", "[inspector] repeated submission → same attempt payment-17"],
        "e23" => &["PASS dialog Escape and submit restore shipping focus", "PASS Pay skipped while quote is pending"],
        "e24" => &["[preview] quote settled · Amsterdam · €84.00", "[preview] Order orbit-1042 confirmed"],
        "e25" => &["$ pnpm exec playwright test checkout.spec.ts", "PASS 6/6 browser checks"],
        "e26" => &["[handoff → Fable] Focus restoration + keyboard evidence"],
        "e27" => &["$ pnpm --filter orders test", "PASS 12/12 · concurrent requests reuse payment-17"],
        "e28" => &["[handoff → Astra] Revision-aware quotes and atomic attempts ready"],
        "e29" => &["[handoff → Astra] Checkout and accessibility changes ready"],
        "e30" => &["$ pnpm test:checkout-smoke", "PASS order orbit-1042 · one payment · €84.00"],
        "e31" => &["[evidence] 6 browser + 12 service + keyboard checks verified"],
        "e32" => &["[session complete] Ready for human review; no merge or deployment"],
        _ => &[],
    }
}

fn diff(id: &str) -> &'static [&'static str] {
    match id {
        "e12" => &["+ const request = ++quoteRequest.current;", "  const next = await fetchQuote(address);", "- setQuote(next);", "+ if (request === quoteRequest.current) setQuote(next);"],
        "e13" => &["- const key = `${basket.id}:${address.postcode}`;", "+ const key = `${basket.id}:${basket.revision}:${address.postcode}`;", "  return quoteCache.getOrLoad(key, () => priceShipping(basket, address));"],
        "e16" => &["+ const trigger = document.activeElement as HTMLElement | null;", "  function closeDialog() {", "    setOpen(false);", "+   trigger?.focus();", "  }"],
        "e20" => &["+ const quoteReady = !quotePending && quote?.revision === basket.revision;", "- <PayButton disabled={submitting} />", "+ <PayButton disabled={submitting || !quoteReady} />", "+ {quotePending && <Status>Updating total…</Status>}"],
        "e21" => &["- return createAttempt(basket.id, amount);", "+ return db.transaction(async tx => {", "+   const key = `${basket.id}:${basket.revision}`;", "+   return tx.attempts.createOrGet({ key, amount });", "+ });"],
        _ => &[],
    }
}

fn app(apps: &mut Vec<AppSnapshot>, id: &'static str, stage: AppStage, frame: u32, caption: &'static str) {
    if let Some(app) = apps.iter_mut().find(|app| app.id == id) {
        app.stage = stage; app.frame = frame; app.caption = caption;
        return;
    }
    let (owner, title, route) = match id {
        "checkout" => ("fable", "Orbit Shop · Checkout", "/checkout"),
        "admin" => ("api", "Orbit Shop · Order inspector", "/admin/orders"),
        "test_runner" => ("qa", "Checkout regression runner", "/__tests/checkout"),
        _ => unreachable!("only fixture app IDs are replayed"),
    };
    apps.push(AppSnapshot { id, owner, title, route, stage, caption, frame });
}

fn update_apps(apps: &mut Vec<AppSnapshot>, id: &str) {
    use AppStage::*;
    match id {
        "e04" => app(apps, "checkout", Starting, 0, "Starting the seeded storefront preview…"),
        "e05" => app(apps, "test_runner", Starting, 0, "Browser regression queued; no result yet."),
        "e06" => app(apps, "admin", Starting, 0, "Connecting the order inspector to the seeded service…"),
        "e07" => app(apps, "checkout", Loading, 1, "Orbit lamp · shipping quote loading · Pay unavailable"),
        "e09" => app(apps, "test_runner", Broken, 1, "1/6 browser checks fails: stale quote replaces current total."),
        "e10" => app(apps, "checkout", Broken, 2, "Amsterdam selected · stale Utrecht total €91.00 · Pay enabled"),
        "e11" => app(apps, "admin", Broken, 1, "Basket orbit-42 · revision 7 · two pending payment attempts"),
        "e14" => app(apps, "test_runner", Loading, 2, "8 quote-contract checks pass; browser and payment checks remain."),
        "e17" => app(apps, "checkout", Fixed, 3, "Amsterdam · €84.00 · latest response retained; retest pending"),
        "e18" => {
            app(apps, "checkout", Broken, 4, "Updating address… · previous €84.00 total · Pay enabled too early");
            app(apps, "test_runner", Broken, 3, "Stale response fixed; pending-quote payment check fails.");
        },
        "e20" => app(apps, "checkout", Loading, 5, "Updating total… · Pay disabled until the current quote settles"),
        "e21" => app(apps, "admin", Loading, 2, "Applying atomic payment-attempt creation…"),
        "e22" => app(apps, "admin", Fixed, 3, "Basket orbit-42 · payment-17 reused · one pending attempt"),
        "e23" => app(apps, "test_runner", Loading, 4, "Keyboard checks pass; full browser and API runs remain."),
        "e24" => app(apps, "checkout", Fixed, 6, "Order orbit-1042 confirmed · €84.00 · final verification pending"),
        "e25" => app(apps, "test_runner", Fixed, 5, "6/6 browser checks pass; API and combined smoke checks remain."),
        "e27" => app(apps, "test_runner", Fixed, 6, "6 browser + 12 API checks pass; combined smoke check remains."),
        "e30" => {
            app(apps, "checkout", Verified, 7, "Order orbit-1042 confirmed · €84.00 · checkout smoke verified");
            app(apps, "admin", Verified, 4, "Order orbit-1042 · one settled payment · €84.00 · verified");
            app(apps, "test_runner", Verified, 7, "6 browser + 12 API + keyboard checks verified; smoke passed.");
        },
        _ => {},
    }
}

/// Seek from the initial state every time. Snapshots contain only agents,
/// results, files and app frames observed by this point in the sample story.
pub fn snapshot(at: f64) -> DemoSnapshot {
    let at = if at.is_finite() { at.clamp(0.0, DURATION) } else { 0.0 };
    let mut snapshot = DemoSnapshot { at, agents: Vec::new(), events: Vec::new(), apps: Vec::new() };
    for event in EVENTS.iter().filter(|event| event.at <= at) {
        if event.kind == EventKind::Spawn { snapshot.agents.push(agent(event.agent)); }
        let agent = snapshot.agents.iter_mut().find(|agent| agent.id == event.agent).expect("fixture event follows agent spawn");
        agent.task = event.title;
        agent.state = match event.kind {
            EventKind::Spawn if event.agent == "qa" => AgentState::Waiting,
            EventKind::Issue => AgentState::Blocked,
            EventKind::Test if matches!(event.id, "e09" | "e18") => AgentState::Blocked,
            EventKind::Test => AgentState::Testing,
            EventKind::Finish => AgentState::Done,
            _ => AgentState::Working,
        };
        if let Some(path) = event.path {
            if agent.file != Some(path) { agent.diff.clear(); }
            agent.file = Some(path);
        }
        if event.kind == EventKind::Edit { agent.diff = diff(event.id).to_vec(); }
        agent.terminal.extend_from_slice(terminal(event.id));
        if agent.terminal.len() > 8 { agent.terminal.drain(..agent.terminal.len() - 8); }
        update_apps(&mut snapshot.apps, event.id);
        snapshot.events.push(*event);
    }
    snapshot.agents.sort_by_key(|agent| ORDER.iter().position(|id| *id == agent.id).unwrap());
    snapshot
}

/// Ground F10 inspection in the current replay frame, never in future results.
pub fn describe(at: f64, selected: Option<&str>) -> String {
    let snapshot = snapshot(at);
    let mut out = format!("SYNTHETIC STUDIO DEMO — Orbit Shop · checkout recovery\nReplay: {:.1}/{DURATION:.0} seconds. These agents, terminal excerpts, files, tests and app frames are sample data, not observed live work. No commands are executed and no files are changed by replay.\n", snapshot.at);
    if let Some(selected) = selected {
        let visible = snapshot.agents.iter().any(|a| a.id == selected)
            || snapshot.apps.iter().any(|a| a.id == selected)
            || snapshot.events.iter().any(|e| e.id == selected);
        let _ = writeln!(out, "Selected: {selected}{}", if visible { "" } else { " (not present at this replay time)" });
    }
    out.push_str("\nCurrent agent work (parent links are explicit sample delegations):\n");
    for agent in &snapshot.agents {
        let _ = writeln!(out, "{} [{}] parent={} · {} · {} · {:?}\nTask: {}", agent.name, agent.id, agent.parent.unwrap_or("none"), agent.role, agent.system, agent.state, agent.task);
        if let Some(file) = agent.file { let _ = writeln!(out, "Open sample file: {file}"); }
        for line in &agent.diff { let _ = writeln!(out, "  {line}"); }
        out.push_str("Terminal excerpt:\n");
        for line in &agent.terminal { let _ = writeln!(out, "  {line}"); }
    }
    out.push_str("\nCurrent synthetic app frames:\n");
    for app in &snapshot.apps {
        let _ = writeln!(out, "{} [{}] owner={} route={} stage={:?} frame={}\n{}", app.title, app.id, app.owner, app.route, app.stage, app.frame, app.caption);
    }
    out.push_str("\nObserved sample timeline:\n");
    for event in &snapshot.events {
        let _ = writeln!(out, "{:>5.1}s [{}] {} {:?}: {}\n{}", event.at, event.id, event.agent, event.kind, event.title, event.detail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn replay_has_no_future_agents_results_or_frames_and_parents_precede_children() {
        let mut ids = HashSet::new();
        let all = events();
        assert!((25..=35).contains(&all.len()));
        assert!(all.windows(2).all(|pair| pair[0].at < pair[1].at));
        for event in &all { assert!(ids.insert(event.id)); }
        for at in 0..=180 {
            let current = snapshot(at as f64);
            let expected: Vec<_> = all.iter().filter(|event| event.at <= at as f64).copied().collect();
            assert_eq!(current.events, expected);
            let mut agents = HashSet::new();
            for agent in &current.agents {
                assert!(agent.parent.is_none_or(|parent| agents.contains(parent)));
                assert!(current.events.iter().any(|event| event.agent == agent.id && event.kind == EventKind::Spawn));
                assert!(agents.insert(agent.id));
                assert!(agent.terminal.len() <= 8);
            }
            for app in &current.apps { assert!(agents.contains(app.owner)); }
        }
        assert_eq!(snapshot(8.0).agents.len(), 1);
        assert!(snapshot(19.0).apps.is_empty());
        assert!(!snapshot(38.0).agents.iter().any(|a| a.id == "a11y"));
        assert!(snapshot(170.0).apps.iter().all(|a| a.stage != AppStage::Verified));
        assert!(!describe(44.0, Some("qa")).contains("all six browser checks"));
        assert!(!describe(60.0, None).contains("quoteRequest.current"));
    }

    #[test]
    fn failure_fix_retest_and_nested_handoff_remain_visible_at_their_times() {
        let early = snapshot(49.0);
        assert_eq!(early.agents.iter().find(|a| a.id == "qa").unwrap().state, AgentState::Blocked);
        assert_eq!(early.apps.iter().find(|a| a.id == "checkout").unwrap().stage, AppStage::Broken);
        assert_eq!(snapshot(91.0).apps.iter().find(|a| a.id == "checkout").unwrap().stage, AppStage::Fixed);
        assert_eq!(snapshot(98.0).apps.iter().find(|a| a.id == "checkout").unwrap().stage, AppStage::Broken);
        let busy = snapshot(120.0);
        assert!(busy.agents.iter().filter(|a| a.state == AgentState::Working).count() >= 3);
        assert!(busy.agents.iter().all(|a| a.state != AgentState::Done));
        assert!(busy.agents.iter().find(|a| a.id == "fable").unwrap().diff.iter().any(|line| line.contains("quoteReady")));
        let nested = snapshot(146.0);
        let a11y = nested.agents.iter().find(|a| a.id == "a11y").unwrap();
        assert_eq!(a11y.parent, Some("fable"));
        assert_eq!(a11y.state, AgentState::Done);
        assert_ne!(nested.agents.iter().find(|a| a.id == "fable").unwrap().state, AgentState::Done);
        let done = snapshot(DURATION);
        assert!(done.agents.iter().all(|a| a.state == AgentState::Done));
        assert_eq!(done.apps.len(), 3);
        assert!(done.apps.iter().all(|a| a.stage == AppStage::Verified));
        assert_eq!(done.events, events());
    }

    #[test]
    fn arbitrary_seek_is_independent_and_invalid_times_are_bounded() {
        let original = snapshot(120.0);
        let mut later = snapshot(DURATION);
        later.agents[0].terminal.clear();
        later.apps.clear();
        assert_eq!(snapshot(120.0), original);
        assert_eq!(snapshot(-1.0), snapshot(0.0));
        assert_eq!(snapshot(999.0), snapshot(DURATION));
        assert_eq!(snapshot(f64::NAN), snapshot(0.0));
        assert_eq!(snapshot(f64::INFINITY), snapshot(0.0));
        let past = snapshot(20.0);
        assert_eq!(past.apps.len(), 1);
        assert_eq!(past.apps[0].frame, 0);
        assert!(past.agents.iter().all(|a| a.diff.is_empty()));
    }
}
