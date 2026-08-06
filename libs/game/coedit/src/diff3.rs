//! Line-level three-way merge.
//!
//! Two Claudes editing one game file produce chunky edits seconds apart, so the
//! merge only has to answer one question: did they touch the same region? When
//! they did not, both edits land. When they did, the caller hands the loser a
//! new base and lets the AI that wrote the change re-derive it — nothing here
//! ever stitches overlapping edits together textually (game.md).

/// Splits on `\n` without dropping the trailing empty field, so
/// `join_lines(&split_lines(s)) == s` for every input including `""`.
pub fn split_lines(source: &str) -> Vec<String> {
    source.split('\n').map(|line| line.to_string()).collect()
}

pub fn join_lines(lines: &[String]) -> String {
    lines.join("\n")
}

/// Longest common subsequence as `(a_index, b_index)` pairs.
///
/// O(n·m) in time and memory, which is the right trade for game sources
/// (a few hundred lines) and is bounded upstream by `Limits::max_source_bytes`.
fn lcs_pairs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let stride = m + 1;
    let mut dp = vec![0u32; (n + 1) * stride];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i * stride + j] = if a[i] == b[j] {
                dp[(i + 1) * stride + j + 1] + 1
            } else {
                dp[(i + 1) * stride + j].max(dp[i * stride + j + 1])
            };
        }
    }

    let mut pairs = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if dp[(i + 1) * stride + j] >= dp[i * stride + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

/// One contiguous edit of `base` into `other`, in base line coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub base_start: usize,
    pub removed: usize,
    pub added: usize,
}

/// The edits that turn `base` into `other`. Used to tell a rebasing author what
/// moved underneath it, not to apply anything.
pub fn hunks(base: &str, other: &str) -> Vec<Hunk> {
    let b = split_lines(base);
    let o = split_lines(other);
    let pairs = lcs_pairs(&b, &o);

    let mut out = Vec::new();
    let (mut bi, mut oi) = (0, 0);
    for (pb, po) in pairs.iter().copied().chain(std::iter::once((b.len(), o.len()))) {
        if pb > bi || po > oi {
            out.push(Hunk {
                base_start: bi,
                removed: pb - bi,
                added: po - oi,
            });
        }
        bi = pb + 1;
        oi = po + 1;
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Every region resolved without guessing.
    Clean(String),
    /// `regions` places where both sides changed the same lines differently.
    /// Deliberately carries no merged text: a half-merged game file that parses
    /// is worse than an honest rejection.
    Conflict { regions: usize },
}

/// Classic diff3: walk the anchors matched in *both* variants, and resolve each
/// region between anchors by which side moved.
pub fn merge3(base: &str, mine: &str, theirs: &str) -> Merge {
    let b = split_lines(base);
    let m = split_lines(mine);
    let t = split_lines(theirs);

    let mut mine_of: Vec<Option<usize>> = vec![None; b.len()];
    for (i, j) in lcs_pairs(&b, &m) {
        mine_of[i] = Some(j);
    }
    let mut theirs_of: Vec<Option<usize>> = vec![None; b.len()];
    for (i, k) in lcs_pairs(&b, &t) {
        theirs_of[i] = Some(k);
    }

    let mut out: Vec<String> = Vec::new();
    let mut conflicts = 0usize;
    let (mut bi, mut mi, mut ti) = (0usize, 0usize, 0usize);

    loop {
        // Next base line both variants still agree on, at or after every cursor.
        let anchor = (bi..b.len()).find_map(|i| match (mine_of[i], theirs_of[i]) {
            (Some(j), Some(k)) if j >= mi && k >= ti => Some((i, j, k)),
            _ => None,
        });
        let (ab, am, at) = anchor.unwrap_or((b.len(), m.len(), t.len()));

        let base_region = &b[bi..ab];
        let mine_region = &m[mi..am];
        let theirs_region = &t[ti..at];

        if !(base_region.is_empty() && mine_region.is_empty() && theirs_region.is_empty()) {
            if mine_region == base_region {
                out.extend_from_slice(theirs_region);
            } else if theirs_region == base_region {
                out.extend_from_slice(mine_region);
            } else if mine_region == theirs_region {
                // Both authors made the identical edit — converged, not a clash.
                out.extend_from_slice(mine_region);
            } else {
                conflicts += 1;
            }
        }

        match anchor {
            Some(_) => {
                out.push(b[ab].clone());
                bi = ab + 1;
                mi = am + 1;
                ti = at + 1;
            }
            None => break,
        }
    }

    if conflicts > 0 {
        Merge::Conflict { regions: conflicts }
    } else {
        Merge::Clean(join_lines(&out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_join_round_trips_exactly() {
        for s in ["", "\n", "a", "a\n", "a\nb", "a\nb\n", "\n\na\n\n"] {
            assert_eq!(join_lines(&split_lines(s)), s, "round trip {s:?}");
        }
    }

    #[test]
    fn disjoint_edits_both_land() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let mine = "ONE\ntwo\nthree\nfour\nfive\n";
        let theirs = "one\ntwo\nthree\nfour\nFIVE\n";
        assert_eq!(
            merge3(base, mine, theirs),
            Merge::Clean("ONE\ntwo\nthree\nfour\nFIVE\n".to_string())
        );
    }

    #[test]
    fn one_sided_edit_is_taken_verbatim() {
        let base = "a\nb\nc\n";
        assert_eq!(
            merge3(base, base, "a\nB\nc\n"),
            Merge::Clean("a\nB\nc\n".to_string())
        );
        assert_eq!(
            merge3(base, "a\nB\nc\n", base),
            Merge::Clean("a\nB\nc\n".to_string())
        );
    }

    #[test]
    fn identical_edits_converge_instead_of_conflicting() {
        let base = "a\nb\nc\n";
        let same = "a\nBEE\nc\n";
        assert_eq!(merge3(base, same, same), Merge::Clean(same.to_string()));
    }

    #[test]
    fn same_region_edited_differently_conflicts() {
        let base = "a\nb\nc\n";
        match merge3(base, "a\nMINE\nc\n", "a\nTHEIRS\nc\n") {
            Merge::Conflict { regions } => assert_eq!(regions, 1),
            other => panic!("expected a conflict, got {other:?}"),
        }
    }

    #[test]
    fn insertions_at_different_places_both_land() {
        let base = "head\nbody\ntail\n";
        let mine = "head\nMINE\nbody\ntail\n";
        let theirs = "head\nbody\ntail\nTHEIRS\n";
        assert_eq!(
            merge3(base, mine, theirs),
            Merge::Clean("head\nMINE\nbody\ntail\nTHEIRS\n".to_string())
        );
    }

    #[test]
    fn deletion_on_one_side_lands() {
        let base = "a\nb\nc\nd\n";
        let mine = "a\nc\nd\n";
        let theirs = "a\nb\nc\nD\n";
        assert_eq!(
            merge3(base, mine, theirs),
            Merge::Clean("a\nc\nD\n".to_string())
        );
    }

    #[test]
    fn delete_versus_modify_of_the_same_line_conflicts() {
        let base = "a\nb\nc\n";
        assert!(matches!(
            merge3(base, "a\nc\n", "a\nCHANGED\nc\n"),
            Merge::Conflict { .. }
        ));
    }

    #[test]
    fn hunks_report_where_the_source_moved() {
        let base = "a\nb\nc\n";
        let other = "a\nB1\nB2\nc\n";
        assert_eq!(
            hunks(base, other),
            vec![Hunk {
                base_start: 1,
                removed: 1,
                added: 2
            }]
        );
        assert!(hunks(base, base).is_empty());
    }

    #[test]
    fn merging_against_an_empty_base_still_terminates() {
        assert!(matches!(merge3("", "a\n", "b\n"), Merge::Conflict { .. }));
        assert_eq!(merge3("", "", ""), Merge::Clean(String::new()));
    }
}
