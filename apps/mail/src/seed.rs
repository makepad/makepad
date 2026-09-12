//! Deterministic fake mail. The same anchor always encodes the same document.

use crate::model::*;
use makepad_civil_time::{add_days, parse_iso};

pub const PEOPLE: &[(&str, &str)] = &[
    ("Alex Morgan", "alex@example.test"),
    ("Maya Chen", "maya.chen@example.test"),
    ("Theo Bakker", "theo.bakker@example.test"),
    ("Lena Ortiz", "lena.ortiz@example.test"),
    ("Samir Patel", "samir.patel@example.test"),
    ("Iris de Vries", "iris.de.vries@example.test"),
    ("Noah Kim", "noah.kim@example.test"),
    ("Priya Shah", "priya.shah@example.test"),
    ("Jules Martin", "jules.martin@example.test"),
    ("Owen Reed", "owen.reed@example.test"),
    ("Sofia Rossi", "sofia.rossi@example.test"),
    ("Northline Rail", "northline.rail@example.test"),
    ("Canal House Hotel", "canal.house.hotel@example.test"),
    ("Papertrail Receipts", "papertrail.receipts@example.test"),
    ("Field Notes Weekly", "field.notes.weekly@example.test"),
    ("Studio Updates", "studio.updates@example.test"),
];

pub const UNREAD: &[u32] = &[
    1, 3, 5, 7, 9, 11, 13, 15, 16, 18, 20, 22, 24, 26, 28, 30, 45, 50, 54, 58,
];
pub const FLAGGED: &[u32] = &[2, 7, 16, 23, 31, 44, 55, 58];

pub fn person(name: &str) -> Address {
    PEOPLE
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(n, e)| Address::new(*n, *e))
        .unwrap_or_else(|| Address::new(name, "unknown@example.test"))
}

fn me() -> Address {
    person("Alex Morgan")
}

pub fn generate(anchor: SeedAnchor) -> MailDocument {
    let today = parse_iso(&anchor.day).unwrap_or(0);
    let midnight = anchor.midnight_utc;
    let alex = me();
    let mut messages = Vec::with_capacity(60);

    push_thread(
        &mut messages,
        midnight,
        today,
        1,
        &[1, 2, 31],
        2,
        "Maya Chen",
        &[
            (
                "Harbor launch checklist — preview ready",
                harbor_body(1, "The first walkthrough of the harbor launch checklist is ready. Please open the preview, check the navigation labels on the three opening screens, and mark anything that still sounds like internal jargon. I left a note beside the hero copy and another beside the footer. If the wording holds, we can lock the deck before Thursday."),
            ),
            (
                "Harbor launch checklist — copy review",
                harbor_body(2, "I folded your first notes into the checklist copy. The remaining question is the title on screen two: \"Launch window\" versus \"Harbor window\". Please pick one and glance at the attached PDF so we share a single source of truth. Once that lands I will freeze the preview."),
            ),
            (
                "Re: Harbor launch checklist — copy review",
                harbor_body(31, "Approved on both counts. Please keep \"Harbor window\" and send the frozen preview to the studio list. I will be in the workshop Thursday morning if anything still snags. Thank you for turning this around so cleanly."),
            ),
        ],
        Some((2, "launch-checklist.pdf", "application/pdf", 186_000)),
    );

    push_thread(
        &mut messages,
        midnight,
        today,
        2,
        &[3, 4, 5, 32],
        8,
        "Lena Ortiz",
        &[
            (
                "Thursday design review — agenda",
                review_body(3, "Here is the agenda for Thursday's design review. We will start with the harbor window, then the Rotterdam workshop boards, then a short look at the autumn equipment shortlist. Please arrive with one concrete question rather than a stack of slides."),
            ),
            (
                "Thursday design review — prototype",
                review_body(4, "The prototype now includes the revised navigation and the two-line mail preview. Please click through the compact layout once on a phone-width window before we meet. I want the room to argue about hierarchy, not missing states."),
            ),
            (
                "Thursday design review — room",
                review_body(5, "We have the canal-side room from ten until noon. Bring the printed boards if you still like paper; the projector is already booked. If you cannot join, send your notes the night before so we do not stall on your section."),
            ),
            (
                "Re: Thursday design review — room",
                review_body(32, "Confirmed, I will be there at ten with the boards and a short note on the compact layout. Please keep my slot to ten minutes so we still have time for the workshop itinerary."),
            ),
        ],
        None,
    );

    push_thread(
        &mut messages,
        midnight,
        today,
        3,
        &[6, 7, 8, 33, 42],
        16,
        "Theo Bakker",
        &[
            (
                "Rotterdam workshop itinerary — rail",
                journey_body(6, "2026-06-12", "Northline has the morning train from Amsterdam Centraal. Please keep the confirmation nearby and travel with a photo of the ticket. I will meet you on the platform if the workshop start stays at eleven."),
            ),
            (
                "Rotterdam workshop itinerary — venue",
                journey_body(7, "2026-06-12", "The venue is a ten-minute walk from Rotterdam Centraal, on the canal side of the studio block. Ask at the desk for the harbor room. Coffee is in the lobby; lunch is around the corner if we run long."),
            ),
            (
                "Rotterdam workshop itinerary — tickets",
                journey_body(8, "2026-06-12", "Tickets are attached. Each file is a single PDF with the outbound and return legs. Please do not screenshot the barcode; the conductor wants the original file. Reply if the name on the ticket is wrong."),
            ),
            (
                "Re: Rotterdam workshop itinerary — tickets",
                journey_body(33, "2026-06-12", "Tickets look right, name included. I will be on the morning train and will text when I reach the platform. Thank you for sorting the venue and the walk from the station."),
            ),
            (
                "Rotterdam workshop itinerary — recap",
                journey_body(42, "2026-06-12", "A short recap from the room: the harbor window copy is locked, the workshop boards need one more pass, and the autumn equipment shortlist will wait a week. I filed the notes in Projects so we can find them after travel."),
            ),
        ],
        Some((8, "rail-tickets.pdf", "application/pdf", 242_000)),
    );

    push_thread(
        &mut messages,
        midnight,
        today,
        4,
        &[9, 10, 34],
        31,
        "Priya Shah",
        &[
            (
                "Autumn studio equipment — shortlist",
                work_body(9, "I trimmed the autumn equipment shortlist to four items we can defend in a review: two lights, a quiet recorder, and the spare monitor. Please look at quantities before we send this upstairs. Nothing here is decorative."),
            ),
            (
                "Autumn studio equipment — quantities",
                work_body(10, "Quantities are now one of each, plus a spare bulb pack. If the monitor can wait until next quarter I will drop it. Otherwise I need a yes this week so purchasing can place the order."),
            ),
            (
                "Re: Autumn studio equipment — quantities",
                work_body(34, "Yes on the lights and the recorder; hold the monitor. Please send the revised shortlist with those two lines and I will sign it. Thank you for keeping the list this small."),
            ),
        ],
        None,
    );

    push_thread(
        &mut messages,
        midnight,
        today,
        5,
        &[11, 12, 35, 43],
        51,
        "Canal House Hotel",
        &[
            (
                "Canal House reservation — dates",
                journey_body(11, "2026-07-03", "We can hold the canal room for the nights you asked about. Please confirm the arrival date and whether you still want the quieter courtyard side. Breakfast is included either way."),
            ),
            (
                "Canal House reservation — room",
                journey_body(12, "2026-07-03", "The courtyard room is reserved. It is one flight up, with a desk that faces the water. Let us know if you need a late check-in; the desk stays open until midnight."),
            ),
            (
                "Re: Canal House reservation — room",
                journey_body(35, "2026-07-03", "The courtyard room is perfect. I will arrive after eight. Please keep the reservation in Alex Morgan's name and send walking directions from Rotterdam Centraal when you can."),
            ),
            (
                "Canal House reservation — directions",
                journey_body(43, "2026-07-03", "From Rotterdam Centraal, take the tram toward the harbor and step off at the canal stop. The hotel is the brick house with the green door. A walking map is attached if you prefer to go on foot."),
            ),
        ],
        None,
    );

    push_thread(
        &mut messages,
        midnight,
        today,
        6,
        &[13, 14, 15, 36, 44],
        82,
        "Jules Martin",
        &[
            (
                "September reading group — shortlist",
                work_body(13, "Three books for the September reading group: a workshop diary, a short book on harbors, and a collection of studio notes. Please vote so we do not spend the first meeting arguing the pile."),
            ),
            (
                "September reading group — votes",
                work_body(14, "Votes are in. The workshop diary leads, the harbor book is second, and the studio notes can wait. If nobody objects by Friday I will order copies and pick a room."),
            ),
            (
                "September reading group — choice",
                work_body(15, "We are reading the workshop diary. I will bring two extra copies. Please finish the first third before we meet so the conversation is about the work, not the table of contents."),
            ),
            (
                "Re: September reading group — choice",
                work_body(36, "I will read the first third and can host if the canal room is free. Please send the date once the venue is booked. Looking forward to this one."),
            ),
            (
                "September reading group — reminder",
                work_body(44, "Reminder: we meet next week in the canal room. The reading list is attached. Bring a passage you want to argue with, not a summary. Tea is on me if you arrive early."),
            ),
        ],
        Some((44, "reading-list.pdf", "application/pdf", 96_000)),
    );

    for id in 1u32..=60 {
        if messages.iter().any(|m| m.id.0 == id) {
            continue;
        }
        messages.push(singleton(id, midnight, today, &alex));
    }

    messages.sort_by_key(|m| m.id.0);

    MailDocument {
        schema_version: SCHEMA_VERSION,
        seed_version: SEED_VERSION,
        seed_anchor: anchor,
        next_message_id: 61,
        next_thread_id: 1061,
        me: alex,
        vip_emails: vec![
            "maya.chen@example.test".into(),
            "lena.ortiz@example.test".into(),
            "iris.de.vries@example.test".into(),
        ],
        messages,
    }
}

fn push_thread(
    out: &mut Vec<Message>,
    midnight: i64,
    today: i32,
    thread: u32,
    ids: &[u32],
    newest_age: i32,
    sender: &str,
    parts: &[(&str, String)],
    attachment: Option<(u32, &str, &str, u64)>,
) {
    let alex = me();
    let from_other = person(sender);
    for (index, id) in ids.iter().enumerate() {
        let age = newest_age + (ids.len() as i32 - 1 - index as i32);
        let sent = matches!(*id, 31..=38);
        let folder = match *id {
            1..=30 => Folder::Inbox,
            31..=38 => Folder::Sent,
            42..=49 => Folder::Archive,
            _ => Folder::Inbox,
        };
        let (subject, body) = parts[index].clone();
        let mut attachments = Vec::new();
        if let Some((aid, name, mime, size)) = attachment {
            if *id == aid {
                attachments.push(Attachment { name: name.into(), mime: mime.into(), size_bytes: size });
            }
        }
        out.push(Message {
            id: MessageId(*id),
            thread_id: ThreadId(thread),
            in_reply_to: if index == 0 { None } else { Some(MessageId(ids[index - 1])) },
            kind: if sent { MessageKind::Sent } else { MessageKind::Received },
            folder,
            from: if sent { alex.clone() } else { from_other.clone() },
            to: vec![if sent { from_other.clone() } else { alex.clone() }],
            cc: Vec::new(),
            subject: subject.into(),
            body_text: body,
            attachments,
            date_secs: date_secs(midnight, today, *id, Some(age)),
            updated_secs: date_secs(midnight, today, *id, Some(age)),
            unread: UNREAD.contains(id),
            flagged: FLAGGED.contains(id),
            draft_input: None,
        });
    }
}

fn singleton(id: u32, midnight: i64, today: i32, alex: &Address) -> Message {
    let folder = folder_of(id);
    let kind = match folder {
        Folder::Drafts => MessageKind::Draft,
        Folder::Sent => MessageKind::Sent,
        _ => MessageKind::Received,
    };
    let people = correspondents();
    let other = people[(id as usize + 3) % people.len()].clone();
    let (subject, body, from, to) = if folder == Folder::Drafts {
        draft_content(id, alex)
    } else if folder == Folder::Junk {
        let (s, b) = junk_content(id);
        (s, b, other.clone(), vec![alex.clone()])
    } else if folder == Folder::Sent {
        let (s, b) = singleton_content(id, folder);
        (s, b, alex.clone(), vec![other.clone()])
    } else {
        let (s, b) = singleton_content(id, folder);
        (s, b, other.clone(), vec![alex.clone()])
    };
    Message {
        id: MessageId(id),
        thread_id: ThreadId(1000 + id),
        in_reply_to: None,
        kind,
        folder,
        from,
        to,
        cc: Vec::new(),
        subject,
        body_text: body,
        attachments: attachment_of(id),
        date_secs: date_secs(midnight, today, id, None),
        updated_secs: date_secs(midnight, today, id, None),
        unread: UNREAD.contains(&id),
        flagged: FLAGGED.contains(&id),
        draft_input: if folder == Folder::Drafts {
            Some(draft_input(id))
        } else {
            None
        },
    }
}

fn folder_of(id: u32) -> Folder {
    match id {
        1..=30 => Folder::Inbox,
        31..=38 => Folder::Sent,
        39..=41 => Folder::Drafts,
        42..=49 => Folder::Archive,
        50..=51 => Folder::Junk,
        52..=53 => Folder::Trash,
        54..=57 => Folder::Projects,
        58..=60 => Folder::Travel,
        _ => Folder::Inbox,
    }
}

fn correspondents() -> Vec<Address> {
    PEOPLE
        .iter()
        .skip(1)
        .map(|(n, e)| Address::new(*n, *e))
        .collect()
}

fn date_secs(_midnight: i64, today: i32, id: u32, age_override: Option<i32>) -> i64 {
    let age = if id == 16 {
        0
    } else if id == 60 {
        89
    } else {
        age_override.unwrap_or(((37 * id as i64 + 11) % 90) as i32)
    };
    let day = add_days(today, -age);
    let midnight_then = day as i64 * 86_400;
    if age == 0 {
        midnight_then
    } else {
        let span = 10 * 3600 - 60;
        let tod = 8 * 3600 + ((id as i64 * 7919 + 13) % span);
        midnight_then + tod
    }
}

fn attachment_of(id: u32) -> Vec<Attachment> {
    let spec = match id {
        2 => Some(("launch-checklist.pdf", "application/pdf", 186_000)),
        8 => Some(("rail-tickets.pdf", "application/pdf", 242_000)),
        19 => Some(("receipt-1019.pdf", "application/pdf", 48_000)),
        27 => Some(("materials.csv", "text/csv", 18 * 1024)),
        44 => Some(("reading-list.pdf", "application/pdf", 96_000)),
        58 => Some(("walking-map.png", "image/png", 480_000)),
        _ => None,
    };
    spec.map(|(name, mime, size)| vec![Attachment { name: name.into(), mime: mime.into(), size_bytes: size }])
        .unwrap_or_default()
}

fn draft_input(id: u32) -> DraftInput {
    match id {
        39 => DraftInput { to_raw: "maya.chen@example.test".into(), cc_raw: String::new() },
        40 => DraftInput { to_raw: String::new(), cc_raw: String::new() },
        _ => DraftInput { to_raw: "theo@".into(), cc_raw: String::new() },
    }
}

fn draft_content(id: u32, alex: &Address) -> (String, String, Address, Vec<Address>) {
    let subject = match id {
        39 => "Harbor window — one last note",
        40 => "Notes from the canal room",
        _ => "Thursday slot",
    };
    let body = match id {
        39 => "Maya, one last note on the harbor window before I send this.\n\n",
        40 => "Still collecting the notes from yesterday.\n\n",
        _ => "Theo, can you take the Thursday slot if I\n\n",
    };
    (subject.into(), body.into(), alex.clone(), Vec::new())
}

fn singleton_content(id: u32, folder: Folder) -> (String, String) {
    if folder == Folder::Projects {
        return work_pattern(id);
    }
    if folder == Folder::Travel {
        return journey_pattern(id);
    }
    match id % 6 {
        0 => work_pattern(id),
        1 => journey_pattern(id),
        2 => receipt_pattern(id),
        3 => field_notes(id),
        4 => lunch_pattern(id),
        _ => materials_pattern(id),
    }
}

fn work_pattern(id: u32) -> (String, String) {
    let n = id;
    (
        format!("Harbor preview — review {n}"),
        harbor_body(
            id,
            "The latest preview is ready. Please review the navigation and the wording on the first screens, then return comments before the next review so we can keep the harbor window on schedule.",
        ),
    )
}

fn journey_pattern(id: u32) -> (String, String) {
    let date = "12 Jun";
    (
        format!("Your Rotterdam journey — {date}"),
        journey_body(
            id,
            "2026-06-12",
            &format!(
                "Departure and arrival details are below. Your reference is NL-{}. Please keep this confirmation with you on the day and show it if the conductor asks.",
                1000 + id
            ),
        ),
    )
}

fn receipt_pattern(id: u32) -> (String, String) {
    let euros = 18 + (id * 7 % 120);
    (
        format!("Receipt {} — studio supplies", 1000 + id),
        format!(
            "Hello Alex,\n\nThank you for the studio supplies order. This is the receipt for reference {}.\n\nThe order totals €{euros}.00 and will be delivered to the studio during ordinary hours. If a line looks wrong, reply with the reference and we will correct it. Nothing in this message is a request for payment; it is only the record of what already shipped.\n\nThe packing list follows the same order as the website cart. Please keep this note with the delivery so the studio shelf matches the invoice.\n\nThanks,\nPapertrail Receipts\n",
            1000 + id
        ),
    )
}

fn field_notes(id: u32) -> (String, String) {
    (
        format!("Field Notes — issue {id}"),
        format!(
            "Hello from Field Notes Weekly.\n\nIssue {id} opens with a short walk along the harbor boards: the studio has been testing two-line mail previews and the compact title still wants more air. The list should feel like a page you can scan with a thumb, not a spreadsheet of metadata. That is the design note for this week, and it will probably still be true next week.\n\nThe workshop note is from Rotterdam. A room that faces the canal changes how people argue. They look at the water when they disagree, then they come back with a smaller sentence. We are stealing that for Thursday's review, and we are leaving one empty chair by the window on purpose. If you have a board that only works when you shout, leave it at the studio.\n\nThe book note is the September reading group. A workshop diary beat a harbor monograph by a handful of votes. Bring a passage you want to argue with, not a summary of the table of contents, and arrive early if you want tea. Extra copies will sit on the sill.\n\nIf this issue was forwarded to you, you can ignore the invitation to reply; it is a newsletter, not a thread. We will write again next week with another three notes, still short, still from the same desk, still with no discounts and no packing list.\n\nUntil then,\nField Notes Weekly\n"
        ),
    )
}

fn lunch_pattern(id: u32) -> (String, String) {
    let date = "18 Sep";
    (
        format!("Lunch near the canal — {date}"),
        format!(
            "Hi Alex,\n\nWould you like lunch near the canal on {date}? I was thinking the small table by the window at noon, or one o'clock if the review runs long. Nothing formal; I want to hear how the harbor window is landing and whether the Rotterdam trip still makes sense for the next workshop.\n\nIf noon is impossible, reply with a better hour and I will move. If you would rather walk first, we can start at the green door and decide when we see the water.\n\nPlease confirm either way so I do not hover in the lobby.\n\nThanks,\n{}\n",
            PEOPLE[(id as usize + 5) % PEOPLE.len()].0
        ),
    )
}

fn materials_pattern(id: u32) -> (String, String) {
    (
        format!("Workshop materials — revision {id}"),
        format!(
            "Hi Alex,\n\nThe workshop materials for revision {id} are prepared. Boards, handouts, and the short brief are in the studio folder. One concrete question remains: should the compact layout keep a large title on the message list, or should we give that height back to the first rows?\n\nPlease answer before Friday so I can print. If you want to see the boards on a phone-width window first, say so and I will send a grab. I would rather fix this on paper than argue it in the room.\n\nThanks,\nStudio Updates\n"
        ),
    )
}

fn junk_content(id: u32) -> (String, String) {
    (
        format!("Limited studio offer — pack {id}"),
        format!(
            "Hello,\n\nThis week only, a generic supply pack is available to studios on our list. No account is required. Tap through to claim a discount that will almost certainly still be there next week.\n\nIf you did not ask for this, you can ignore it. We will send another one anyway.\n\nPromotions desk\n"
        ),
    )
}

fn harbor_body(id: u32, premise: &str) -> String {
    format!(
        "Hi Alex,\n\n{premise}\n\nI would like comments back before the next review so the harbor window stays on the Thursday agenda. Nothing here needs a meeting of its own; a short reply is enough. If a screen still feels like internal jargon, mark it and I will rewrite that line. Please keep this with the rest of the studio notes so it does not get lost in the thread.\n\nThanks,\n{}\n",
        if id == 31 { "Alex Morgan" } else { "Maya Chen" }
    )
}

fn review_body(id: u32, premise: &str) -> String {
    format!(
        "Hi Alex,\n\n{premise}\n\nPlease treat this as the working plan unless you send a change. I will keep the room to two hours and stop on time even if we still have opinions left. Bring the thing you want decided, not a tour of the whole file. A short note the night before is better than a surprise in the room.\n\nThanks,\n{}\n",
        if id == 32 { "Alex Morgan" } else { "Lena Ortiz" }
    )
}

fn journey_body(id: u32, date: &str, premise: &str) -> String {
    let from = match id {
        33 | 35 => "Alex Morgan",
        6 | 7 | 8 | 42 => "Theo Bakker",
        _ => "Canal House Hotel",
    };
    format!(
        "Hello Alex,\n\n{premise}\n\nTravel date {date}. Reference NL-{}. Keep this confirmation with you on the day and show it if anyone at the desk asks. If the time shifts I will write again rather than change this thread in place. A short reply is enough if everything looks right.\n\nRegards,\n{from}\n",
        1000 + id
    )
}

fn work_body(id: u32, premise: &str) -> String {
    let from = if matches!(id, 34 | 36) {
        "Alex Morgan"
    } else if (13..=15).contains(&id) || id == 44 {
        "Jules Martin"
    } else {
        "Priya Shah"
    };
    format!(
        "Hi Alex,\n\n{premise}\n\nPlease reply with a yes, a no, or a smaller version. I would rather have a short decision than a long discussion that we repeat next week. The studio can move as soon as this is settled. Keep this note with the rest of the project mail so we can find the decision later.\n\nThanks,\n{from}\n"
    )
}

pub fn test_anchor() -> SeedAnchor {
    let day = parse_iso("2026-09-09").unwrap();
    SeedAnchor { day: "2026-09-09".into(), midnight_utc: day as i64 * 86_400 }
}

#[cfg(test)]
pub fn test_doc() -> MailDocument {
    generate(test_anchor())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{mailbox_messages, validate_document};
    use crate::storage::{decode_document, encode_document};
    use std::collections::BTreeSet;

    fn words(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn seed_is_deterministic_and_complete() {
        let a = generate(test_anchor());
        let b = generate(test_anchor());
        assert_eq!(encode_document(&a).unwrap(), encode_document(&b).unwrap());
        validate_document(&a).unwrap();
        assert_eq!(a.messages.len(), 60);
        assert_eq!(a.messages.iter().map(|m| m.id.0).collect::<BTreeSet<_>>().len(), 60);
        assert!(a.messages.iter().all(|m| (1..=60).contains(&m.id.0)));
        assert_eq!(a.next_message_id, 61);
        assert_eq!(a.next_thread_id, 1061);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Inbox), false).len(), 30);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Sent), false).len(), 8);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Drafts), false).len(), 3);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Archive), false).len(), 8);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Junk), false).len(), 2);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Trash), false).len(), 2);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Projects), false).len(), 4);
        assert_eq!(mailbox_messages(&a, Mailbox::Folder(Folder::Travel), false).len(), 3);
        let externals: BTreeSet<_> = a
            .messages
            .iter()
            .filter(|m| m.from.email != a.me.email)
            .map(|m| m.from.email.as_str())
            .collect();
        assert_eq!(externals.len(), 15);
        let thread_size = |t| a.messages.iter().filter(|m| m.thread_id == ThreadId(t)).count();
        assert_eq!(thread_size(1), 3);
        assert_eq!(thread_size(2), 4);
        assert_eq!(thread_size(3), 5);
        assert_eq!(thread_size(4), 3);
        assert_eq!(thread_size(5), 4);
        assert_eq!(thread_size(6), 5);
        let min = a.messages.iter().map(|m| m.date_secs).min().unwrap();
        let max = a.messages.iter().map(|m| m.date_secs).max().unwrap();
        assert_eq!(max, a.seed_anchor.midnight_utc);
        assert_eq!(max.div_euclid(86_400) - min.div_euclid(86_400), 89);
        for id in UNREAD {
            assert!(a.message(MessageId(*id)).unwrap().unread, "unread {id}");
        }
        for id in FLAGGED {
            assert!(a.message(MessageId(*id)).unwrap().flagged, "flagged {id}");
        }
        for (id, name) in [
            (2u32, "launch-checklist.pdf"),
            (8, "rail-tickets.pdf"),
            (19, "receipt-1019.pdf"),
            (27, "materials.csv"),
            (44, "reading-list.pdf"),
            (58, "walking-map.png"),
        ] {
            let at = &a.message(MessageId(id)).unwrap().attachments;
            assert_eq!(at[0].name, name);
            assert!((18 * 1024..481 * 1024).contains(&at[0].size_bytes));
        }
        let sixteen = a.message(MessageId(16)).unwrap();
        assert_eq!(sixteen.date_secs, a.seed_anchor.midnight_utc);
        let sixty = a.message(MessageId(60)).unwrap();
        assert_eq!(sixty.date_secs.div_euclid(86_400), a.seed_anchor.midnight_utc / 86_400 - 89);
        for m in &a.messages {
            if m.folder == Folder::Junk {
                continue;
            }
            if m.folder == Folder::Drafts {
                assert!(words(&m.body_text) < 60);
                continue;
            }
            let n = words(&m.body_text);
            if m.subject.starts_with("Field Notes") {
                assert!((180..=240).contains(&n), "newsletter {} has {n} words", m.id);
            } else {
                assert!((60..=140).contains(&n), "message {} has {n} words", m.id);
            }
        }
        assert_eq!(a.message(MessageId(39)).unwrap().draft_input.as_ref().unwrap().to_raw, "maya.chen@example.test");
        assert_eq!(a.message(MessageId(40)).unwrap().draft_input.as_ref().unwrap().to_raw, "");
        assert_eq!(a.message(MessageId(41)).unwrap().draft_input.as_ref().unwrap().to_raw, "theo@");
        let _ = decode_document(&encode_document(&a).unwrap()).unwrap();
    }
}
