use crate::{
    change::{ChangeKind, Drift},
    iter::IteratorExt,
    Change, Point, Range, Selection, Text,
};

pub fn insert(
    text: &mut Text,
    selections: &[Selection],
    additional_text: Text,
    changes: &mut Vec<Change>,
) {
    let mut origin = Point::zero();
    let mut adjusted_origin = Point::zero();
    for range in selections
        .iter()
        .copied()
        .merge(
            |selection_0, selection_1| match selection_0.merge(selection_1) {
                Some(selection) => Ok(selection),
                None => Err((selection_0, selection_1)),
            },
        )
        .map(|selection| selection.range())
    {
        let adjusted_start = adjusted_origin + (range.start() - origin);
        let adjusted_range = Range::from_start_and_extent(adjusted_start, range.extent());
        let change = Change {
            drift: Drift::Before,
            kind: ChangeKind::Delete(adjusted_range),
        };
        text.apply_change(change.clone());
        changes.push(change);
        let change = Change {
            drift: Drift::Before,
            kind: ChangeKind::Insert(adjusted_range.start(), additional_text.clone()),
        };
        text.apply_change(change.clone());
        changes.push(change);
        origin = range.end();
        adjusted_origin = adjusted_start + additional_text.extent();
    }
}

pub fn delete(text: &mut Text, selections: &[Selection], changes: &mut Vec<Change>) {
    let mut origin = Point::zero();
    let mut adjusted_origin = Point::zero();
    for range in selections
        .iter()
        .copied()
        .merge(
            |selection_0, selection_1| match selection_0.merge(selection_1) {
                Some(selection) => Ok(selection),
                None => Err((selection_0, selection_1)),
            },
        )
        .map(|selection| selection.range())
    {
        let adjusted_start = adjusted_origin + (range.start() - origin);
        let adjusted_range = Range::from_start_and_extent(adjusted_start, range.extent());
        let change = Change {
            drift: Drift::Before,
            kind: ChangeKind::Delete(adjusted_range),
        };
        text.apply_change(change.clone());
        changes.push(change);
        origin = range.end();
        adjusted_origin = adjusted_start;
    }
}
