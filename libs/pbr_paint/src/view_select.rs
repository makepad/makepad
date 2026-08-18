//! Exact port of the upstream greedy bake-view selection
//! (`hy3dpaint/utils/pipeline_utils.py::ViewProcessor.bake_view_selection`).
//!
//! The first six candidates are always selected. Then, while below
//! `max_selected`, the candidate adding the largest *new* visible face-area
//! ratio is selected if that increment exceeds 0.01; ties keep the earliest
//! candidate (upstream uses a strict `>` when scanning in index order).
//! Rendering is decoupled: callers supply per-candidate visible face sets
//! (upstream renders 1024x1024 face-index alpha maps for this).

use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq)]
pub struct SelectionResult {
    /// Indices into the candidate arrays, in selection order.
    pub selected: Vec<usize>,
    /// Total visible face-area ratio covered by the selection.
    pub covered_area_ratio: f64,
}

/// `face_area_ratios[f]` is face f's area divided by total mesh area.
/// `visible_faces[c]` is the set of face indices candidate view c sees.
pub fn bake_view_selection(
    face_area_ratios: &[f64],
    visible_faces: &[BTreeSet<u32>],
    max_selected: usize,
) -> SelectionResult {
    assert!(
        visible_faces.len() >= 6,
        "candidate set must include the six canonical views"
    );
    let area_of = |faces: &BTreeSet<u32>| -> f64 {
        faces
            .iter()
            .map(|&f| face_area_ratios.get(f as usize).copied().unwrap_or(0.0))
            .sum()
    };

    let mut selected = Vec::new();
    let mut is_selected = vec![false; visible_faces.len()];
    let mut union: BTreeSet<u32> = BTreeSet::new();

    for idx in 0..6 {
        selected.push(idx);
        is_selected[idx] = true;
        union.extend(visible_faces[idx].iter().copied());
    }
    let mut covered = area_of(&union);

    while selected.len() < max_selected {
        let mut max_inc = 0.0f64;
        let mut max_idx = None;
        for (idx, faces) in visible_faces.iter().enumerate() {
            if is_selected[idx] {
                continue;
            }
            let inc: f64 = faces
                .iter()
                .filter(|f| !union.contains(f))
                .map(|&f| face_area_ratios.get(f as usize).copied().unwrap_or(0.0))
                .sum();
            if inc > max_inc {
                max_inc = inc;
                max_idx = Some(idx);
            }
        }
        match max_idx {
            Some(idx) if max_inc > 0.01 => {
                is_selected[idx] = true;
                selected.push(idx);
                union.extend(visible_faces[idx].iter().copied());
                covered += max_inc;
            }
            _ => break,
        }
    }

    SelectionResult {
        selected,
        covered_area_ratio: covered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(faces: &[u32]) -> BTreeSet<u32> {
        faces.iter().copied().collect()
    }

    /// 10 faces of equal ratio 0.1; six canonical views each see face 0..=3,
    /// candidate 6 adds faces 4..=8 (0.5 new), candidate 7 adds face 9 (0.1),
    /// candidate 8 adds nothing new.
    fn fixture() -> (Vec<f64>, Vec<BTreeSet<u32>>) {
        let ratios = vec![0.1; 10];
        let mut vis = vec![set(&[0, 1, 2, 3]); 6];
        vis.push(set(&[2, 3, 4, 5, 6, 7, 8]));
        vis.push(set(&[0, 9]));
        vis.push(set(&[1, 2]));
        (ratios, vis)
    }

    #[test]
    fn first_six_always_selected() {
        let (ratios, vis) = fixture();
        let result = bake_view_selection(&ratios, &vis, 6);
        assert_eq!(result.selected, vec![0, 1, 2, 3, 4, 5]);
        assert!((result.covered_area_ratio - 0.4).abs() < 1e-12);
    }

    #[test]
    fn greedy_adds_largest_new_area_first() {
        let (ratios, vis) = fixture();
        let result = bake_view_selection(&ratios, &vis, 9);
        // 6 canonical + candidate 6 (0.5 new) + candidate 7 (0.1 new);
        // candidate 8 adds 0.0 which is not > 0.01, so selection stops.
        assert_eq!(result.selected, vec![0, 1, 2, 3, 4, 5, 6, 7]);
        assert!((result.covered_area_ratio - 1.0).abs() < 1e-12);
    }

    #[test]
    fn threshold_blocks_tiny_increments() {
        let ratios = vec![0.001; 1000];
        let mut vis = vec![set(&[0]); 6];
        vis.push(set(&[1, 2, 3])); // adds 0.003 < 0.01
        let result = bake_view_selection(&ratios, &vis, 9);
        assert_eq!(result.selected.len(), 6);
    }

    #[test]
    fn cap_respected_and_ties_keep_first() {
        let ratios = vec![0.05; 20];
        let mut vis = vec![set(&[0]); 6];
        vis.push(set(&[1])); // +0.05
        vis.push(set(&[2])); // +0.05 (tie -> candidate 6 wins first)
        let result = bake_view_selection(&ratios, &vis, 7);
        assert_eq!(result.selected, vec![0, 1, 2, 3, 4, 5, 6]);
    }
}
