//! Deterministic sample notes. Generated only when storage is missing.

use crate::model::{FolderId, Note, NoteId, NotesDocument, DOCUMENT_VERSION, SEED_VERSION};

const HOUR_MS: i64 = 60 * 60 * 1000;
const MINUTE_MS: i64 = 60 * 1000;

struct SeedRow {
    id: u64,
    folder: FolderId,
    age_hours: i64,
    deleted: bool,
    pinned: bool,
    title: &'static str,
    body: &'static str,
}

const ROWS: [SeedRow; 20] = [
    SeedRow {
        id: 1,
        folder: FolderId::Notes,
        age_hours: 2,
        deleted: false,
        pinned: true,
        title: "Weekend groceries",
        body: "Saturday market before lunch.\n- [ ] Tomatoes and basil\n- [x] Coffee beans\n- [ ] Oat milk\n- [ ] Sourdough",
    },
    SeedRow {
        id: 2,
        folder: FolderId::Notes,
        age_hours: 5,
        deleted: false,
        pinned: false,
        title: "Lemon pasta",
        body: "Serves two. Keep a mug of pasta water.\n1. Zest one lemon.\n2. Warm butter and black pepper.\n3. Toss pasta with lemon, water and parmesan.",
    },
    SeedRow {
        id: 3,
        folder: FolderId::Notes,
        age_hours: 26,
        deleted: false,
        pinned: false,
        title: "Reading queue",
        body: "- A book about urban trees\n- The essay Maya recommended\n- Finish the chapter on colour before buying another book.",
    },
    SeedRow {
        id: 4,
        folder: FolderId::Notes,
        age_hours: 52,
        deleted: false,
        pinned: false,
        title: "Balcony measurements",
        body: "Clear width: 238 cm. Door swing needs 72 cm.\n# Shelving\nKeep the top shelf below the window latch.",
    },
    SeedRow {
        id: 5,
        folder: FolderId::Notes,
        age_hours: 96,
        deleted: false,
        pinned: false,
        title: "Coffee places to try",
        body: "- Harbour café: filter coffee and a quiet back room\n- Corner bakery: go before 10\nAsk Noor which place roasts its own beans.",
    },
    SeedRow {
        id: 6,
        folder: FolderId::Notes,
        age_hours: 240,
        deleted: false,
        pinned: false,
        title: "Gift ideas",
        body: "Sam: a small sketchbook and soft pencils.\nLina: tickets for a ceramics workshop.\n**Check dates before booking.**",
    },
    SeedRow {
        id: 7,
        folder: FolderId::Work,
        age_hours: 1,
        deleted: false,
        pinned: true,
        title: "Thursday product review",
        body: "Bring the prototype and three open questions.\n# Decisions\nKeep the first-run screen short.\n## Follow-up\n- [ ] Maya: review the empty state\n- [ ] Jules: check keyboard navigation\n- [x] Send the agenda",
    },
    SeedRow {
        id: 8,
        folder: FolderId::Work,
        age_hours: 7,
        deleted: false,
        pinned: false,
        title: "Launch checklist",
        body: "Aim for a quiet, reversible release.\n- [x] Confirm scope\n- [x] Review copy\n- [ ] Check narrow layout\n- [ ] Test restore\n- [ ] Verify saved edits\n- [ ] Review error states\n- [ ] Prepare release notes\n- [ ] Name the support contact\n- [ ] Final smoke check",
    },
    SeedRow {
        id: 9,
        folder: FolderId::Work,
        age_hours: 28,
        deleted: false,
        pinned: false,
        title: "Interview notes — café owner",
        body: "Orders arrive in bursts between 08:30 and 10:00.\nStaff need to see the next action immediately.\n*“A short queue feels manageable.”*\nFollow up about closing routines.",
    },
    SeedRow {
        id: 10,
        folder: FolderId::Work,
        age_hours: 55,
        deleted: false,
        pinned: false,
        title: "Workshop agenda",
        body: "1. Introductions — 10 minutes\n2. Current workflow — 20 minutes\n3. Sketch alternatives — 25 minutes\n4. Agree next steps — 5 minutes",
    },
    SeedRow {
        id: 11,
        folder: FolderId::Work,
        age_hours: 120,
        deleted: false,
        pinned: false,
        title: "Copy review",
        body: "Replace “Submit request” with “Send”.\nKeep error messages beside the affected field.\nExplain what happens after an empty search.",
    },
    SeedRow {
        id: 12,
        folder: FolderId::Work,
        age_hours: 360,
        deleted: false,
        pinned: false,
        title: "Team retrospective",
        body: "# Keep\nSmall reviews early in the week.\n# Change\nLeave a clear owner on every follow-up.\n# Try\nA five-minute demo before planning.",
    },
    SeedRow {
        id: 13,
        folder: FolderId::Personal,
        age_hours: 3,
        deleted: false,
        pinned: false,
        title: "Coastal weekend",
        body: "Train on Friday afternoon; return Sunday evening.\n# Pack\n- [ ] Rain jacket\n- [ ] Walking shoes\n- [x] Chargers\n# Saturday\nBreakfast near the station. Walk the harbour loop.\nLeave the afternoon unplanned.",
    },
    SeedRow {
        id: 14,
        folder: FolderId::Personal,
        age_hours: 22,
        deleted: false,
        pinned: false,
        title: "Bike maintenance",
        body: "Rear tyre loses pressure slowly. Check the valve first.\n- [ ] Clean and oil chain\n- [ ] Inspect brake pads\n- [x] Tighten front light",
    },
    SeedRow {
        id: 15,
        folder: FolderId::Personal,
        age_hours: 49,
        deleted: false,
        pinned: false,
        title: "Sunday lunch",
        body: "Six people, one vegetarian.\nMake the roasted vegetables ahead.\nAsk Eli to bring bread; keep dessert simple.",
    },
    SeedRow {
        id: 16,
        folder: FolderId::Personal,
        age_hours: 76,
        deleted: false,
        pinned: false,
        title: "Garden diary",
        body: "The mint has new shoots. Rosemary needs less water.\nMove the small pots away from the afternoon sun.",
    },
    SeedRow {
        id: 17,
        folder: FolderId::Personal,
        age_hours: 144,
        deleted: false,
        pinned: false,
        title: "Apartment projects",
        body: "1. Replace the hallway bulb.\n2. Measure curtains before ordering.\n3. Patch the mark behind the desk.",
    },
    SeedRow {
        id: 18,
        folder: FolderId::Personal,
        age_hours: 480,
        deleted: false,
        pinned: false,
        title: "Language practice",
        body: "Ten minutes after breakfast.\nSay each phrase aloud before writing it.\nThis week: directions, ordering lunch and arranging a meeting.",
    },
    SeedRow {
        id: 19,
        folder: FolderId::Notes,
        age_hours: 200,
        deleted: true,
        pinned: false,
        title: "Old shopping list",
        body: "Rice, apples and washing-up liquid.\nReplaced by the weekend list.",
    },
    SeedRow {
        id: 20,
        folder: FolderId::Work,
        age_hours: 300,
        deleted: true,
        pinned: false,
        title: "Superseded meeting agenda",
        body: "Initial discussion outline.\nThe agreed agenda is in Thursday product review.",
    },
];

/// Round the supplied current time down to a whole minute.
pub fn seed_anchor_ms(now_ms: i64) -> i64 {
    now_ms - now_ms.rem_euclid(MINUTE_MS)
}

/// Build the 20-note document for a missing storage jail.
pub fn generate(now_ms: i64) -> NotesDocument {
    let anchor = seed_anchor_ms(now_ms);
    let mut notes = Vec::with_capacity(ROWS.len());
    for row in ROWS {
        let edited_ms = anchor - row.age_hours * HOUR_MS;
        let created_ms = edited_ms - 48 * HOUR_MS;
        let deleted_ms = if row.deleted { Some(anchor - 12 * HOUR_MS) } else { None };
        notes.push(Note {
            id: NoteId(row.id),
            folder: row.folder,
            source: format!("{}\n\n{}", row.title, row.body),
            pinned: row.pinned,
            created_ms,
            edited_ms,
            deleted_ms,
        });
    }
    NotesDocument {
        version: DOCUMENT_VERSION,
        seed_version: SEED_VERSION,
        seed_anchor_ms: anchor,
        next_id: 21,
        revision: 1,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{active_count, folder_counts, pinned_active};
    use crate::model::Collection;
    use std::collections::HashSet;

    #[test]
    fn seed_is_repeatable_and_counts_match() {
        let a = generate(1_788_955_230_000);
        let b = generate(1_788_955_259_999);
        assert_eq!(a.seed_anchor_ms, b.seed_anchor_ms);
        assert_eq!(a.notes, b.notes);
        assert_eq!(a.next_id, 21);
        let ids: HashSet<_> = a.notes.iter().map(|n| n.id).collect();
        assert_eq!(ids.len(), 20);
        assert_eq!(active_count(&a), 18);
        let counts = folder_counts(&a);
        assert_eq!(counts.iter().find(|c| c.collection == Collection::All).unwrap().count, 18);
        assert_eq!(
            counts.iter().find(|c| c.collection == Collection::Folder(FolderId::Notes)).unwrap().count,
            6
        );
        assert_eq!(
            counts.iter().find(|c| c.collection == Collection::Folder(FolderId::Work)).unwrap().count,
            6
        );
        assert_eq!(
            counts.iter().find(|c| c.collection == Collection::Folder(FolderId::Personal)).unwrap().count,
            6
        );
        assert_eq!(
            counts.iter().find(|c| c.collection == Collection::RecentlyDeleted).unwrap().count,
            2
        );
        let pins = pinned_active(&a);
        assert_eq!(pins.len(), 2);
        assert!(pins.contains(&NoteId(1)));
        assert!(pins.contains(&NoteId(7)));
        assert!(a.notes.iter().find(|n| n.id == NoteId(1)).unwrap().pinned);
        assert!(a.notes.iter().find(|n| n.id == NoteId(7)).unwrap().pinned);
        let n1 = a.notes.iter().find(|n| n.id == NoteId(1)).unwrap();
        assert_eq!(n1.created_ms, n1.edited_ms - 48 * HOUR_MS);
        let deleted = a.notes.iter().find(|n| n.id == NoteId(19)).unwrap();
        assert_eq!(deleted.deleted_ms, Some(a.seed_anchor_ms - 12 * HOUR_MS));
    }
}
