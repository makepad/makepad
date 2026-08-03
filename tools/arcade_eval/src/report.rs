//! Result records and the markdown report.
//!
//! The aggregate that matters is the ranked failure list: it names what the
//! model misunderstood, which is the input to the next context edit.

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Pending,
    Pass,
    /// Generated, ran, but did not build what was asked (reason).
    Failed(String),
    /// The script did not evaluate at all (the error).
    EvalError(String),
    /// The agent never produced a file.
    NoFile(String),
    /// The game hard-crashed the engine. The worst outcome: a kid's prompt
    /// should never be able to take the process down.
    Panicked,
    Skipped(String),
}

pub struct Run {
    pub slug: &'static str,
    pub prompt: &'static str,
    pub outcome: Outcome,
    pub gen_secs: f64,
    pub turns: usize,
    pub tool_calls: usize,
    pub lines: usize,
    pub entities: usize,
    pub entities_after: usize,
    pub verbs: Vec<String>,
    pub runtime_error: Option<String>,
    pub cached: bool,
}

impl Run {
    pub fn new(slug: &'static str, prompt: &'static str) -> Self {
        Self {
            slug,
            prompt,
            outcome: Outcome::Pending,
            gen_secs: 0.0,
            turns: 0,
            tool_calls: 0,
            lines: 0,
            entities: 0,
            entities_after: 0,
            verbs: Vec::new(),
            runtime_error: None,
            cached: false,
        }
    }

    pub fn passed(&self) -> bool {
        self.outcome == Outcome::Pass
    }

    /// Did it reach for an engine block rather than hand-rolling movement?
    pub fn used_blocks(&self) -> bool {
        const BLOCKS: &[&str] = &[
            "car",
            "character",
            "plane",
            "wander",
            "chase",
            "patrol",
            "checkpoint",
            "race",
            "spawnpoint",
        ];
        self.verbs.iter().any(|v| BLOCKS.contains(&v.as_str()))
    }

    pub fn one_line(&self) -> String {
        match &self.outcome {
            Outcome::Pass => format!(
                "PASS  {:>5.1}s {:>3} lines {:>3} ent {}",
                self.gen_secs,
                self.lines,
                self.entities,
                if self.used_blocks() { "blocks" } else { "hand-rolled" }
            ),
            Outcome::Failed(why) => format!("FAIL  {why}"),
            Outcome::EvalError(e) => format!("EVAL  {}", e.lines().next().unwrap_or("")),
            Outcome::NoFile(e) => format!("NOGEN {e}"),
            Outcome::Panicked => "PANIC engine crashed while running this game".into(),
            Outcome::Skipped(e) => format!("SKIP  {e}"),
            Outcome::Pending => "PENDING".into(),
        }
    }
}

pub fn render(tag: &str, runs: &[Run]) -> String {
    let mut s = format!("# arcade-eval — {tag}\n\n");
    let counted: Vec<&Run> = runs
        .iter()
        .filter(|r| !matches!(r.outcome, Outcome::Skipped(_)))
        .collect();
    let pass = counted.iter().filter(|r| r.passed()).count();
    let evalerr = counted
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::EvalError(_)))
        .count();
    let nogen = counted
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::NoFile(_)))
        .count();
    let panics = counted
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Panicked))
        .count();
    let blocks = counted.iter().filter(|r| r.used_blocks()).count();
    let n = counted.len().max(1);

    s.push_str(&format!(
        "**{pass}/{} passed** · {evalerr} failed to evaluate · {panics} crashed the engine · \
         {nogen} never generated · {blocks}/{} used engine blocks\n\n",
        counted.len(),
        counted.len()
    ));

    let mut times: Vec<f64> = counted
        .iter()
        .filter(|r| r.gen_secs > 0.0)
        .map(|r| r.gen_secs)
        .collect();
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if !times.is_empty() {
        let med = times[times.len() / 2];
        s.push_str(&format!(
            "Generation: median {:.0}s, fastest {:.0}s, slowest {:.0}s (n={})\n\n",
            med,
            times[0],
            times[times.len() - 1],
            times.len()
        ));
    }
    let _ = n;

    s.push_str("| prompt | result | s | lines | ent | blocks | verbs |\n");
    s.push_str("|---|---|--:|--:|--:|---|---|\n");
    for r in runs {
        let verbs = if r.verbs.len() > 8 {
            format!("{}, +{}", r.verbs[..8].join(", "), r.verbs.len() - 8)
        } else {
            r.verbs.join(", ")
        };
        s.push_str(&format!(
            "| {} | {} | {:.0} | {} | {} | {} | {} |\n",
            r.slug,
            r.one_line(),
            r.gen_secs,
            r.lines,
            r.entities,
            if r.used_blocks() { "yes" } else { "no" },
            verbs
        ));
    }

    let failures: Vec<&Run> = counted.iter().filter(|r| !r.passed()).copied().collect();
    if !failures.is_empty() {
        s.push_str("\n## Failures\n\n");
        for r in &failures {
            // The prompt is quoted here because a failure is only readable
            // next to what was actually asked for.
            s.push_str(&format!(
                "- **{}** — _{}_\n  - {}\n",
                r.slug,
                r.prompt,
                r.one_line()
            ));
            if let Some(e) = &r.runtime_error {
                s.push_str(&format!("  - runtime: `{}`\n", e.lines().next().unwrap_or("")));
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(slug: &'static str, outcome: Outcome, verbs: &[&str]) -> Run {
        let mut r = Run::new(slug, "p");
        r.outcome = outcome;
        r.verbs = verbs.iter().map(|v| v.to_string()).collect();
        r
    }

    #[test]
    fn skipped_runs_do_not_count_against_the_score() {
        let runs = vec![
            run("a", Outcome::Pass, &["car"]),
            run("b", Outcome::Skipped("no assets".into()), &[]),
        ];
        let md = render("t", &runs);
        assert!(md.contains("**1/1 passed**"), "{md}");
    }

    #[test]
    fn block_use_is_detected_not_guessed() {
        assert!(run("a", Outcome::Pass, &["car", "box"]).used_blocks());
        assert!(!run("b", Outcome::Pass, &["box", "walk", "mover"]).used_blocks());
    }
}
