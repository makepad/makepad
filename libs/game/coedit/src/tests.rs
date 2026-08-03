use super::*;

const GAME: &str = "world {\n  ground: flat\n}\ncars {\n  count: 4\n}\nrules {\n  laps: 3\n}\n";

fn tx(author: u64, base: u64, intent: &str, source: &str) -> Transaction {
    Transaction {
        author: AuthorId(author),
        intent: intent.to_string(),
        base_generation: base,
        source: source.to_string(),
    }
}

/// Replace the first line containing `needle`.
fn edit(source: &str, needle: &str, replacement: &str) -> String {
    let mut lines = diff3::split_lines(source);
    let at = lines
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not in source"));
    lines[at] = replacement.to_string();
    diff3::join_lines(&lines)
}

// ------------------------------------------------------------------ merging

#[test]
fn two_authors_editing_disjoint_regions_both_land() {
    let mut host = CoeditHost::new(GAME);

    let a = edit(GAME, "count: 4", "  count: 8");
    let b = edit(GAME, "laps: 3", "  laps: 5");

    let first = host.submit(tx(1, 0, "more cars", &a));
    assert!(matches!(first, Outcome::Accepted { generation: 1, .. }));

    // Author 2 wrote against generation 0 and never saw the car change.
    let second = host.submit(tx(2, 0, "longer race", &b));
    let Outcome::Accepted { generation, source } = second else {
        panic!("disjoint edits must merge, got {second:?}");
    };
    assert_eq!(generation, 2);
    assert!(source.contains("count: 8"), "author 1's edit survived");
    assert!(source.contains("laps: 5"), "author 2's edit landed");
}

#[test]
fn the_same_region_edited_twice_rebases_the_second_author() {
    let mut host = CoeditHost::new(GAME);

    host.submit(tx(1, 0, "eight cars", &edit(GAME, "count: 4", "  count: 8")));
    let outcome = host.submit(tx(2, 0, "two cars", &edit(GAME, "count: 4", "  count: 2")));

    let Outcome::Rebase {
        generation,
        base_source,
        intervening,
        conflict_regions,
    } = outcome
    else {
        panic!("overlapping edits must rebase, got {outcome:?}");
    };
    assert_eq!(generation, 1, "the base moved to generation 1");
    assert!(base_source.contains("count: 8"), "the new base is handed back");
    assert_eq!(conflict_regions, 1);
    assert_eq!(intervening.len(), 1, "one generation landed underneath");
    assert_eq!(intervening[0].author, AuthorId(1));
    assert_eq!(intervening[0].intent, "eight cars");
    assert!(!intervening[0].hunks.is_empty(), "the summary says where it moved");
}

#[test]
fn re_applying_an_intent_after_a_rebase_succeeds() {
    let mut host = CoeditHost::new(GAME);
    host.submit(tx(1, 0, "eight cars", &edit(GAME, "count: 4", "  count: 8")));

    let Outcome::Rebase {
        generation,
        base_source,
        ..
    } = host.submit(tx(2, 0, "two cars", &edit(GAME, "count: 4", "  count: 2")))
    else {
        panic!("expected a rebase");
    };

    // What the losing Claude does: re-derive the same intent on the new base.
    let redone = edit(&base_source, "count: 8", "  count: 2");
    let outcome = host.submit(tx(2, generation, "two cars", &redone));

    let Outcome::Accepted { generation, source } = outcome else {
        panic!("the rebased resubmit must land, got {outcome:?}");
    };
    assert_eq!(generation, 2);
    assert!(source.contains("count: 2"));
}

#[test]
fn an_edit_already_present_in_the_tip_is_not_recorded_twice() {
    let mut host = CoeditHost::new(GAME);
    let changed = edit(GAME, "count: 4", "  count: 8");
    host.submit(tx(1, 0, "eight cars", &changed));

    // Author 2 independently made the identical change against the old base.
    let outcome = host.submit(tx(2, 0, "eight cars too", &changed));
    assert_eq!(
        outcome,
        Outcome::Refused {
            reason: Refusal::NoChange
        }
    );
    assert_eq!(host.head().number, 1, "no empty generation was appended");
}

// ------------------------------------------------------------------ history

#[test]
fn the_history_is_linear_and_append_only_under_interleaved_submits() {
    let mut host = CoeditHost::new(GAME);
    let mut expected_head = GAME.to_string();

    for round in 0..12u64 {
        let author = 1 + (round % 3);
        // Everyone writes against a stale base on purpose.
        let base = host.head().number.saturating_sub(round % 2);
        let base_source = host.history().get(base).unwrap().source.clone();
        let mut lines = diff3::split_lines(&base_source);
        lines.push(format!("note_{round}: {author}"));
        let source = diff3::join_lines(&lines);

        if let Outcome::Accepted { source, .. } = host.submit(tx(author, base, "note", &source)) {
            expected_head = source;
        }
    }

    let numbers: Vec<u64> = host.history().iter().map(|g| g.number).collect();
    let linear: Vec<u64> = (0..host.history().len() as u64).collect();
    assert_eq!(numbers, linear, "generations must be 0..n with no gaps");
    assert_eq!(host.head().source, expected_head);
    assert_eq!(
        host.history().get(0).unwrap().source,
        GAME,
        "generation 0 is immutable"
    );
}

// --------------------------------------------------------------- eval errors

#[test]
fn an_eval_error_routes_to_the_proposer_and_the_world_stays_on_last_good() {
    let mut host = CoeditHost::new(GAME);
    host.note_eval_ok(0);

    let good = edit(GAME, "count: 4", "  count: 8");
    let Outcome::Accepted { generation, .. } = host.submit(tx(1, 0, "eight cars", &good)) else {
        panic!("expected acceptance");
    };
    host.note_eval_ok(generation);
    assert_eq!(host.last_good_generation(), 1);

    let broken = edit(&good, "laps: 3", "  laps: definitely_not_a_number");
    let Outcome::Accepted { generation, .. } = host.submit(tx(2, 1, "endless race", &broken)) else {
        panic!("expected acceptance");
    };

    let report = host
        .note_eval_error(generation, "game.splash:7:9: expected a number")
        .expect("the generation exists");
    assert_eq!(report.author, AuthorId(2), "the proposer hears about it");
    assert_eq!(report.intent, "endless race");
    assert_eq!(report.last_good_generation, 1);
    assert!(report.message.contains("expected a number"));

    assert_eq!(
        host.last_good_source(),
        good,
        "the room keeps playing the last good generation"
    );
    assert_eq!(
        host.head().number,
        2,
        "the broken text stays in history so its author can fix it"
    );
}

#[test]
fn a_fixed_generation_becomes_the_new_last_good() {
    let mut host = CoeditHost::new(GAME);
    let broken = edit(GAME, "laps: 3", "  laps: oops");
    let Outcome::Accepted { generation, .. } = host.submit(tx(1, 0, "break it", &broken)) else {
        panic!("expected acceptance");
    };
    host.note_eval_error(generation, "boom");
    assert_eq!(host.last_good_generation(), 0);

    let fixed = edit(&broken, "laps: oops", "  laps: 9");
    let Outcome::Accepted { generation, .. } = host.submit(tx(1, 1, "fix it", &fixed)) else {
        panic!("expected acceptance");
    };
    host.note_eval_ok(generation);
    assert_eq!(host.last_good_generation(), 2);
    assert_eq!(host.last_good_source(), fixed);
}

// ------------------------------------------------------------------- refusals

#[test]
fn malformed_and_oversized_transactions_are_refused_with_a_reason() {
    let limits = Limits {
        max_source_bytes: 256,
        max_intent_bytes: 32,
        ..Limits::default()
    };
    let mut host = CoeditHost::with_limits(GAME, limits);

    assert_eq!(
        host.submit(tx(1, 0, "   ", "x\n")),
        Outcome::Refused {
            reason: Refusal::EmptyIntent
        }
    );
    assert_eq!(
        host.submit(tx(1, 0, &"i".repeat(33), "x\n")),
        Outcome::Refused {
            reason: Refusal::IntentTooLong
        }
    );
    assert_eq!(
        host.submit(tx(1, 0, "big", &"x\n".repeat(200))),
        Outcome::Refused {
            reason: Refusal::SourceTooLong
        }
    );
    assert_eq!(
        host.submit(tx(1, 99, "from nowhere", "x\n")),
        Outcome::Refused {
            reason: Refusal::UnknownBase
        }
    );
    assert_eq!(
        host.submit(tx(1, 0, "no-op", GAME)),
        Outcome::Refused {
            reason: Refusal::NoChange
        }
    );
    assert_eq!(host.head().number, 0, "nothing refused was recorded");
}

#[test]
fn one_author_cannot_fill_the_intake_queue() {
    let limits = Limits {
        max_pending_per_author: 2,
        ..Limits::default()
    };
    let mut host = CoeditHost::with_limits(GAME, limits);

    assert!(host.enqueue(tx(1, 0, "a", "a\n")).is_ok());
    assert!(host.enqueue(tx(1, 0, "b", "b\n")).is_ok());
    assert_eq!(host.enqueue(tx(1, 0, "c", "c\n")), Err(Refusal::QueueFull));
    // A different author is unaffected by its neighbour's backlog.
    assert!(host.enqueue(tx(2, 0, "d", "d\n")).is_ok());

    let outcomes = host.process_queue();
    assert_eq!(outcomes.len(), 3);
    assert_eq!(host.queued(), 0);
}

#[test]
fn a_departing_author_leaves_nothing_behind() {
    let mut host = CoeditHost::new(GAME);
    host.enqueue(tx(7, 0, "queued", "x\n")).unwrap();
    host.leases().acquire(AuthorId(7), "vehicles", 30.0, 0.0);

    host.forget_author(AuthorId(7));

    assert_eq!(host.queued(), 0);
    assert!(host.leases().active(0.0).is_empty());
}

// --------------------------------------------------------------------- leases

#[test]
fn leases_are_advisory_and_never_block_a_submit() {
    let mut host = CoeditHost::new(GAME);
    assert!(matches!(
        host.leases().acquire(AuthorId(1), "cars", 30.0, 0.0),
        LeaseOutcome::Granted { .. }
    ));
    assert!(matches!(
        host.leases().acquire(AuthorId(2), "cars", 30.0, 1.0),
        LeaseOutcome::Held { by: AuthorId(1), .. }
    ));

    // Author 2 submits anyway: the lease only shapes who *chooses* to edit.
    let outcome = host.submit(tx(2, 0, "two cars", &edit(GAME, "count: 4", "  count: 2")));
    assert!(
        matches!(outcome, Outcome::Accepted { .. }),
        "a soft lease must not gate the log, got {outcome:?}"
    );
}

// ---------------------------------------------------------------------- fuzz

/// Deterministic xorshift — no external crates, and a failure reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// A line-level patch with the actual replacement text, so a test can replay
/// the chain of accepted diffs independently of the stored sources.
fn patch(base: &str, target: &str) -> Vec<(usize, usize, Vec<String>)> {
    let b = diff3::split_lines(base);
    let t = diff3::split_lines(target);
    let mut ops = Vec::new();
    for hunk in diff3::hunks(base, target) {
        // Re-derive the added text by aligning the hunk against the target.
        let before = b[..hunk.base_start].to_vec();
        let consumed: usize = ops
            .iter()
            .map(|(_, removed, added): &(usize, usize, Vec<String>)| {
                added.len() as isize - *removed as isize
            })
            .sum::<isize>()
            .max(-(before.len() as isize)) as usize;
        let target_start = hunk.base_start + consumed;
        let added = t[target_start..(target_start + hunk.added).min(t.len())].to_vec();
        ops.push((hunk.base_start, hunk.removed, added));
    }
    ops
}

fn apply(base: &str, ops: &[(usize, usize, Vec<String>)]) -> String {
    let b = diff3::split_lines(base);
    let mut out: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    for (start, removed, added) in ops {
        out.extend_from_slice(&b[cursor..*start]);
        out.extend_from_slice(added);
        cursor = start + removed;
    }
    out.extend_from_slice(&b[cursor..]);
    diff3::join_lines(&out)
}

#[test]
fn concurrent_authors_never_corrupt_the_history() {
    let mut host = CoeditHost::new(GAME);
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let mut accepted = 0usize;
    let mut rebased = 0usize;

    for round in 0..200u64 {
        let author = AuthorId(1 + rng.next() % 4);
        // Authors routinely write against stale bases, which is the whole point.
        let head = host.head().number;
        let base = head.saturating_sub(rng.next() % 3);
        let base_source = host.history().get(base).unwrap().source.clone();

        let mut lines = diff3::split_lines(&base_source);
        match rng.below(3) {
            0 if lines.len() > 2 => {
                let at = rng.below(lines.len());
                lines[at] = format!("edit_{round} by {}", author.0);
            }
            1 => {
                let at = rng.below(lines.len());
                lines.insert(at, format!("insert_{round} by {}", author.0));
            }
            _ if lines.len() > 3 => {
                let at = rng.below(lines.len());
                lines.remove(at);
            }
            _ => lines.push(format!("append_{round}")),
        }
        let source = diff3::join_lines(&lines);

        match host.submit(tx(author.0, base, &format!("round {round}"), &source)) {
            Outcome::Accepted { .. } => accepted += 1,
            Outcome::Rebase { generation, base_source, .. } => {
                rebased += 1;
                // Re-derive on the fresh base, exactly as a Claude would.
                let mut lines = diff3::split_lines(&base_source);
                lines.push(format!("rebased_{round} by {}", author.0));
                let redone = diff3::join_lines(&lines);
                if let Outcome::Accepted { .. } =
                    host.submit(tx(author.0, generation, "rebased", &redone))
                {
                    accepted += 1;
                }
            }
            Outcome::Refused { .. } => {}
        }
    }

    assert!(accepted > 50, "the scenario must actually exercise merges");
    assert!(rebased > 0, "the scenario must actually exercise rebases");

    // Linear, append-only, no gaps.
    let numbers: Vec<u64> = host.history().iter().map(|g| g.number).collect();
    assert_eq!(numbers, (0..host.history().len() as u64).collect::<Vec<_>>());

    // Every generation names a base that already existed when it landed.
    for generation in host.history().iter() {
        assert!(
            generation.base_generation < generation.number.max(1),
            "generation {} claims a base from the future",
            generation.number
        );
    }

    // Replay: applying the chain of accepted diffs from generation 0 must
    // reproduce the head source exactly.
    let mut replayed = host.history().get(0).unwrap().source.clone();
    for number in 1..host.history().len() as u64 {
        let previous = &host.history().get(number - 1).unwrap().source;
        let current = &host.history().get(number).unwrap().source;
        let ops = patch(previous, current);
        replayed = apply(&replayed, &ops);
        assert_eq!(
            &replayed,
            current,
            "replaying accepted diffs diverged at generation {number}"
        );
    }
    assert_eq!(replayed, host.head().source);
}
