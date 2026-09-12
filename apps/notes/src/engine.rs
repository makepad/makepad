//! Pure notes engine: derive, mutate, format, search, layout, navigation.
//! No widget or window types. All time enters as parameters.

use crate::model::*;
use makepad_civil_time::{self as civil, MONTH_ABBR};

const DAY_MS: i64 = 86_400_000;

pub fn normalize_newlines(source: &str) -> String {
    source.replace("\r\n", "\n").replace('\r', "\n")
}

pub fn first_line(source: &str) -> &str {
    source.split('\n').next().unwrap_or(source)
}

pub fn body_after_title(source: &str) -> &str {
    match source.find('\n') {
        Some(i) => &source[i + 1..],
        None => "",
    }
}

/// Display title: trim, strip an optional leading `# ` or `## `, empty → "New Note".
pub fn display_title(source: &str) -> String {
    let mut line = first_line(source).trim();
    if let Some(rest) = line.strip_prefix("## ") {
        line = rest.trim();
    } else if let Some(rest) = line.strip_prefix("# ") {
        line = rest.trim();
    }
    if line.is_empty() {
        "New Note".to_string()
    } else {
        line.to_string()
    }
}

fn strip_markers(line: &str) -> String {
    let mut s = line.trim();
    if let Some(rest) = s.strip_prefix("## ") {
        s = rest;
    } else if let Some(rest) = s.strip_prefix("# ") {
        s = rest;
    } else if let Some(rest) = s
        .strip_prefix("- [ ] ")
        .or_else(|| s.strip_prefix("- [x] "))
    {
        s = rest;
    } else if let Some(rest) = s.strip_prefix("- ") {
        s = rest;
    } else if let Some(rest) = strip_number_prefix(s) {
        s = rest;
    }
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            let ch = s[i + 1..].chars().next().unwrap();
            out.push(ch);
            i += 1 + ch.len_utf8();
            continue;
        }
        if bytes[i] == b'*' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out.trim().to_string()
}

fn strip_number_prefix(s: &str) -> Option<&str> {
    let mut digits = 0;
    for (i, b) in s.bytes().enumerate() {
        if b.is_ascii_digit() {
            digits += 1;
            continue;
        }
        if digits > 0 && b == b'.' && s[i + 1..].starts_with(' ') {
            return Some(&s[i + 2..]);
        }
        break;
    }
    None
}

/// First nonempty body line with supported markers removed.
pub fn display_preview(source: &str) -> String {
    let body = body_after_title(source);
    for line in body.lines() {
        let stripped = strip_markers(line);
        if !stripped.is_empty() {
            return stripped;
        }
    }
    "No additional text".to_string()
}

pub fn plain_body(source: &str) -> String {
    strip_markers(&body_after_title(source).replace('\n', " "))
}

pub fn utc_day(ms: i64) -> civil::Day {
    ms.div_euclid(DAY_MS) as civil::Day
}

pub fn format_clock_hm(ms: i64) -> String {
    let tod = ms.rem_euclid(DAY_MS);
    let mins = (tod / 60_000) as i64;
    format!("{:02}:{:02}", mins / 60, mins % 60)
}

pub fn format_note_date(edited_ms: i64, now_ms: i64) -> String {
    let day = utc_day(edited_ms);
    let today = utc_day(now_ms);
    if day == today {
        format_clock_hm(edited_ms)
    } else if day == today - 1 {
        "Yesterday".to_string()
    } else {
        let (y, m, d) = civil::to_ymd(day);
        let (ny, _, _) = civil::to_ymd(today);
        let month = MONTH_ABBR[(m.saturating_sub(1) % 12) as usize];
        if y == ny {
            format!("{d} {month}")
        } else {
            format!("{d} {month} {y}")
        }
    }
}

pub fn date_section_for(edited_ms: i64, now_ms: i64) -> DateSection {
    let day = utc_day(edited_ms);
    let today = utc_day(now_ms);
    let age = today - day;
    if age <= 0 {
        DateSection::Today
    } else if age == 1 {
        DateSection::Yesterday
    } else if age <= 7 {
        DateSection::Previous7Days
    } else {
        DateSection::Older
    }
}

pub fn active_count(doc: &NotesDocument) -> usize {
    doc.notes.iter().filter(|n| !n.is_deleted()).count()
}

pub fn pinned_active(doc: &NotesDocument) -> Vec<NoteId> {
    doc.notes
        .iter()
        .filter(|n| n.pinned && !n.is_deleted())
        .map(|n| n.id)
        .collect()
}

pub fn folder_counts(doc: &NotesDocument) -> Vec<FolderCount> {
    Collection::ALL
        .into_iter()
        .map(|collection| FolderCount {
            collection,
            count: match collection {
                Collection::All => active_count(doc),
                Collection::Folder(f) => doc
                    .notes
                    .iter()
                    .filter(|n| !n.is_deleted() && n.folder == f)
                    .count(),
                Collection::RecentlyDeleted => doc.notes.iter().filter(|n| n.is_deleted()).count(),
            },
        })
        .collect()
}

pub fn in_collection(note: &Note, collection: Collection) -> bool {
    match collection {
        Collection::All => !note.is_deleted(),
        Collection::Folder(f) => !note.is_deleted() && note.folder == f,
        Collection::RecentlyDeleted => note.is_deleted(),
    }
}

fn matches_query(note: &Note, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    display_title(&note.source).to_lowercase().contains(&q)
        || plain_body(&note.source).to_lowercase().contains(&q)
        || note.source.to_lowercase().contains(&q)
}

fn sort_active(a: &Note, b: &Note) -> std::cmp::Ordering {
    b.edited_ms.cmp(&a.edited_ms).then(a.id.cmp(&b.id))
}

fn sort_deleted(a: &Note, b: &Note) -> std::cmp::Ordering {
    b.deleted_ms.cmp(&a.deleted_ms).then(a.id.cmp(&b.id))
}

pub fn list_hits<'a>(doc: &'a NotesDocument, collection: Collection, query: &str) -> Vec<&'a Note> {
    let mut notes: Vec<&Note> = doc
        .notes
        .iter()
        .filter(|n| in_collection(n, collection) && matches_query(n, query))
        .collect();
    if collection == Collection::RecentlyDeleted {
        notes.sort_by(|a, b| sort_deleted(a, b));
    } else {
        notes.sort_by(|a, b| sort_active(a, b));
    }
    notes
}

pub fn to_hit(note: &Note) -> SearchHit {
    SearchHit {
        id: note.id,
        title: display_title(&note.source),
        preview: display_preview(&note.source),
        folder: note.folder,
        edited_ms: note.edited_ms,
        pinned: note.pinned,
    }
}

pub fn list_rows(
    doc: &NotesDocument,
    collection: Collection,
    query: &str,
    now_ms: i64,
) -> Vec<ListRow> {
    let hits = list_hits(doc, collection, query);
    if hits.is_empty() {
        return Vec::new();
    }
    if collection == Collection::RecentlyDeleted {
        let mut rows = vec![ListRow::Section(DateSection::Deleted)];
        rows.extend(hits.into_iter().map(|n| ListRow::Note(to_hit(n))));
        return rows;
    }
    let mut rows = Vec::new();
    let (pinned, rest): (Vec<&Note>, Vec<&Note>) = hits.into_iter().partition(|n| n.pinned);
    if !pinned.is_empty() {
        rows.push(ListRow::Section(DateSection::Pinned));
        rows.extend(pinned.into_iter().map(|n| ListRow::Note(to_hit(n))));
    }
    let mut groups: Vec<(DateSection, Vec<&Note>)> = Vec::new();
    for note in rest {
        let section = date_section_for(note.edited_ms, now_ms);
        if let Some(last) = groups.last_mut() {
            if last.0 == section {
                last.1.push(note);
                continue;
            }
        }
        groups.push((section, vec![note]));
    }
    // Keep canonical order of sections even if notes arrive mixed after pin split.
    let order = [
        DateSection::Today,
        DateSection::Yesterday,
        DateSection::Previous7Days,
        DateSection::Older,
    ];
    let mut by_section: Vec<(DateSection, Vec<&Note>)> =
        order.into_iter().map(|s| (s, Vec::new())).collect();
    for (section, notes) in groups {
        if let Some((_, slot)) = by_section.iter_mut().find(|(s, _)| *s == section) {
            slot.extend(notes);
        }
    }
    for (section, notes) in by_section {
        if notes.is_empty() {
            continue;
        }
        rows.push(ListRow::Section(section));
        rows.extend(notes.into_iter().map(|n| ListRow::Note(to_hit(n))));
    }
    rows
}

pub fn first_result_id(doc: &NotesDocument, collection: Collection, query: &str) -> Option<NoteId> {
    let hits = list_hits(doc, collection, query);
    if collection != Collection::RecentlyDeleted {
        if let Some(note) = hits.iter().find(|n| n.pinned) {
            return Some(note.id);
        }
    }
    hits.first().map(|n| n.id)
}

pub fn decide_layout(width: f64, height: f64, previous: Option<LayoutDecision>) -> LayoutDecision {
    if width <= 1.0 {
        return previous.unwrap_or(LayoutDecision::DEFAULT);
    }
    let short_chrome = height < 360.0;
    if width < 700.0 {
        return LayoutDecision {
            family: Family::Compact,
            columns: Columns::Stack,
            short_chrome,
        };
    }
    let columns = if width >= 1100.0 && height >= 420.0 {
        Columns::Three
    } else {
        Columns::Two
    };
    LayoutDecision {
        family: Family::Wide,
        columns,
        short_chrome,
    }
}

pub fn list_column_width(decision: LayoutDecision, total_width: f64) -> f64 {
    match decision.columns {
        Columns::Three => 320.0,
        Columns::Two => (total_width * 0.34).min(280.0),
        Columns::Stack => total_width,
    }
}

fn source_len_ok(source: &str) -> bool {
    source.len() <= MAX_SOURCE_BYTES
}

fn aggregate_ok(doc: &NotesDocument, extra: isize) -> bool {
    let total: usize = doc.notes.iter().map(|n| n.source.len()).sum();
    (total as isize + extra) as usize <= MAX_AGGREGATE_SOURCE_BYTES
}

fn bump_revision(doc: &mut NotesDocument) {
    doc.revision = doc.revision.saturating_add(1);
}

pub fn touch_edited(note: &mut Note, now_ms: i64) {
    note.edited_ms = now_ms.max(note.edited_ms);
}

pub fn set_source(
    doc: &mut NotesDocument,
    session: &mut EditSession,
    source: String,
    selection: ByteSelection,
    now_ms: i64,
    kind: EditKind,
) -> Result<(), &'static str> {
    let source = normalize_newlines(&source);
    if !source_len_ok(&source) {
        return Err("note is too large");
    }
    let Some(note) = doc.note(session.note_id) else {
        return Err("note not found");
    };
    if note.is_deleted() {
        return Err("deleted notes cannot be edited");
    }
    let extra = source.len() as isize - note.source.len() as isize;
    if extra > 0 && !aggregate_ok(doc, extra) {
        return Err("notes are too large");
    }
    let prev = EditSnapshot {
        source: note.source.clone(),
        selection: session.selection,
    };
    let Some(note) = doc.note_mut(session.note_id) else {
        return Err("note not found");
    };
    let coalesce = kind == EditKind::Typing
        && session
            .last_type_ms
            .is_some_and(|t| now_ms - t <= UNDO_COALESCE_MS)
        && session.last_type_end == Some(session.selection.end())
        && selection.start() >= session.selection.end().saturating_sub(8);
    if !coalesce {
        session.undo.push(prev);
        if session.undo.len() > MAX_UNDO {
            session.undo.remove(0);
        }
        session.redo.clear();
    }
    if kind == EditKind::Typing {
        session.last_type_ms = Some(now_ms);
        session.last_type_end = Some(selection.end());
    } else {
        session.last_type_ms = None;
        session.last_type_end = None;
    }
    note.source = source;
    session.selection = selection.clamp(note.source.len());
    touch_edited(note, now_ms);
    bump_revision(doc);
    Ok(())
}

pub fn undo(doc: &mut NotesDocument, session: &mut EditSession, now_ms: i64) -> bool {
    let Some(snap) = session.undo.pop() else {
        return false;
    };
    let Some(note) = doc.note_mut(session.note_id) else {
        return false;
    };
    session.redo.push(EditSnapshot {
        source: note.source.clone(),
        selection: session.selection,
    });
    if session.redo.len() > MAX_UNDO {
        session.redo.remove(0);
    }
    note.source = snap.source;
    session.selection = snap.selection.clamp(note.source.len());
    session.last_type_ms = None;
    session.last_type_end = None;
    touch_edited(note, now_ms);
    bump_revision(doc);
    true
}

pub fn redo(doc: &mut NotesDocument, session: &mut EditSession, now_ms: i64) -> bool {
    let Some(snap) = session.redo.pop() else {
        return false;
    };
    let Some(note) = doc.note_mut(session.note_id) else {
        return false;
    };
    session.undo.push(EditSnapshot {
        source: note.source.clone(),
        selection: session.selection,
    });
    if session.undo.len() > MAX_UNDO {
        session.undo.remove(0);
    }
    note.source = snap.source;
    session.selection = snap.selection.clamp(note.source.len());
    session.last_type_ms = None;
    session.last_type_end = None;
    touch_edited(note, now_ms);
    bump_revision(doc);
    true
}

pub fn create_note(
    doc: &mut NotesDocument,
    ui: &mut NotesUi,
    now_ms: i64,
) -> Result<NoteId, &'static str> {
    if ui.collection == Collection::RecentlyDeleted {
        return Err("cannot create in Recently Deleted");
    }
    if doc.notes.len() >= MAX_NOTES {
        return Err("too many notes");
    }
    if doc.next_id > i64::MAX as u64 {
        return Err("id range exhausted");
    }
    let id = NoteId(doc.next_id);
    doc.next_id += 1;
    let folder = ui.collection.create_folder();
    doc.notes.push(Note {
        id,
        folder,
        source: String::new(),
        pinned: false,
        created_ms: now_ms,
        edited_ms: now_ms,
        deleted_ms: None,
    });
    bump_revision(doc);
    ui.query.clear();
    ui.selected = Some(id);
    ui.editor = Some(EditSession {
        note_id: id,
        mode: EditorMode::Edit,
        selection: ByteSelection::caret(0),
        undo: Vec::new(),
        redo: Vec::new(),
        last_type_ms: None,
        last_type_end: None,
    });
    ensure_editor_in_route(ui);
    Ok(id)
}

pub fn pin_toggle(doc: &mut NotesDocument, id: NoteId) -> Result<bool, &'static str> {
    let Some(note) = doc.note_mut(id) else {
        return Err("note not found");
    };
    if note.is_deleted() {
        return Err("cannot pin a deleted note");
    }
    note.pinned = !note.pinned;
    let pinned = note.pinned;
    bump_revision(doc);
    Ok(pinned)
}

pub fn move_note(
    doc: &mut NotesDocument,
    id: NoteId,
    folder: FolderId,
) -> Result<(), &'static str> {
    let Some(note) = doc.note_mut(id) else {
        return Err("note not found");
    };
    if note.is_deleted() {
        return Err("cannot move a deleted note");
    }
    note.folder = folder;
    bump_revision(doc);
    Ok(())
}

pub fn delete_note(
    doc: &mut NotesDocument,
    ui: &mut NotesUi,
    id: NoteId,
    now_ms: i64,
) -> Result<(), &'static str> {
    let Some(note) = doc.note_mut(id) else {
        return Err("note not found");
    };
    if note.is_deleted() {
        return Err("already deleted");
    }
    note.deleted_ms = Some(now_ms.max(note.created_ms));
    note.pinned = false;
    bump_revision(doc);
    if ui.selected == Some(id) {
        ui.selected = first_result_id(doc, ui.collection, &ui.query);
        open_selected_read(doc, ui);
    }
    Ok(())
}

pub fn restore_note(
    doc: &mut NotesDocument,
    ui: &mut NotesUi,
    id: NoteId,
) -> Result<(), &'static str> {
    let Some(note) = doc.note_mut(id) else {
        return Err("note not found");
    };
    if note.deleted_ms.is_none() {
        return Err("not deleted");
    }
    note.deleted_ms = None;
    bump_revision(doc);
    if ui.selected == Some(id) {
        ui.selected = first_result_id(doc, ui.collection, &ui.query);
        open_selected_read(doc, ui);
    }
    Ok(())
}

pub fn select_collection(doc: &NotesDocument, ui: &mut NotesUi, collection: Collection) {
    ui.collection = collection;
    ui.selected = first_result_id(doc, collection, &ui.query);
    open_selected_read(doc, ui);
    if ui.route.last() == Some(&Screen::Folders) {
        ui.route.push(Screen::List);
    } else if !ui.route.contains(&Screen::List) {
        ui.route = vec![Screen::Folders, Screen::List];
    }
}

pub fn select_note(doc: &NotesDocument, ui: &mut NotesUi, id: NoteId) {
    if doc.note(id).is_none() {
        return;
    }
    ui.selected = Some(id);
    open_selected_read(doc, ui);
    ensure_editor_in_route(ui);
}

pub fn set_query(doc: &NotesDocument, ui: &mut NotesUi, query: String) {
    ui.query = query;
    if let Some(id) = ui.selected {
        if list_hits(doc, ui.collection, &ui.query)
            .iter()
            .any(|n| n.id == id)
        {
            return;
        }
    }
    ui.selected = first_result_id(doc, ui.collection, &ui.query);
    open_selected_read(doc, ui);
}

pub fn open_selected_read(doc: &NotesDocument, ui: &mut NotesUi) {
    match ui.selected.and_then(|id| doc.note(id)) {
        Some(note) => {
            let keep_history = ui.editor.as_ref().is_some_and(|e| e.note_id == note.id);
            if keep_history {
                if let Some(editor) = ui.editor.as_mut() {
                    editor.mode = EditorMode::Read;
                }
            } else {
                ui.editor = Some(EditSession::new(note.id));
            }
        }
        None => ui.editor = None,
    }
}

fn ensure_editor_in_route(ui: &mut NotesUi) {
    if ui.route.last() == Some(&Screen::Editor) {
        return;
    }
    if !ui.route.contains(&Screen::List) {
        ui.route = vec![Screen::Folders, Screen::List];
    }
    ui.route.push(Screen::Editor);
}

pub fn go_back(ui: &mut NotesUi) {
    match ui.route.last().copied() {
        Some(Screen::Editor) => {
            if let Some(editor) = ui.editor.as_mut() {
                editor.mode = EditorMode::Read;
            }
            ui.route.pop();
            if ui.route.is_empty() {
                ui.route.push(Screen::Folders);
            }
        }
        Some(Screen::List) => {
            ui.route.pop();
            if ui.route.is_empty() {
                ui.route.push(Screen::Folders);
            }
        }
        _ => {
            if ui.route.is_empty() {
                ui.route.push(Screen::Folders);
            }
        }
    }
}

pub fn current_screen(ui: &NotesUi) -> Screen {
    ui.route.last().copied().unwrap_or(Screen::Folders)
}

pub fn normalize_after_filter(doc: &NotesDocument, ui: &mut NotesUi) {
    if let Some(id) = ui.selected {
        if list_hits(doc, ui.collection, &ui.query)
            .iter()
            .any(|n| n.id == id)
        {
            return;
        }
    }
    ui.selected = first_result_id(doc, ui.collection, &ui.query);
    if ui.route.last() == Some(&Screen::Editor) && ui.selected.is_none() {
        go_back(ui);
        ui.editor = None;
    } else {
        open_selected_read(doc, ui);
    }
}

fn line_spans(source: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            spans.push((start, i));
            start = i + 1;
        }
    }
    spans.push((start, source.len()));
    spans
}

fn selected_line_range(source: &str, sel: ByteSelection) -> (usize, usize) {
    let spans = line_spans(source);
    if spans.is_empty() {
        return (0, 0);
    }
    let line_at = |pos: usize| -> usize {
        for (i, (ls, le)) in spans.iter().enumerate() {
            if pos < *ls {
                return i.saturating_sub(1);
            }
            if pos <= *le {
                return i;
            }
        }
        spans.len() - 1
    };
    if sel.is_empty() {
        let i = line_at(sel.cursor.min(source.len()));
        return (i, i);
    }
    let start = sel.start().min(source.len());
    let end = sel.end().min(source.len());
    let first = line_at(start);
    let last = if end > start && spans.iter().any(|(ls, _)| *ls == end) {
        line_at(end).saturating_sub(1)
    } else if end > start {
        line_at(end.saturating_sub(1))
    } else {
        first
    };
    (first, last.max(first))
}

fn numbered_prefix(line: &str) -> Option<&str> {
    let mut digits = 0;
    for (i, b) in line.bytes().enumerate() {
        if b.is_ascii_digit() {
            digits += 1;
            continue;
        }
        if digits > 0 && b == b'.' && line[i + 1..].starts_with(' ') {
            return Some(&line[..i + 2]);
        }
        break;
    }
    None
}

fn checklist_prefix(line: &str) -> Option<&str> {
    if line.starts_with("- [ ] ") {
        Some("- [ ] ")
    } else if line.starts_with("- [x] ") {
        Some("- [x] ")
    } else {
        None
    }
}

fn recognized_prefix(line: &str) -> Option<&str> {
    if line.starts_with("## ") {
        Some("## ")
    } else if line.starts_with("# ") {
        Some("# ")
    } else if let Some(p) = checklist_prefix(line) {
        Some(p)
    } else if line.starts_with("- ") {
        Some("- ")
    } else {
        numbered_prefix(line)
    }
}

fn strip_block_prefix(line: &str) -> &str {
    recognized_prefix(line)
        .map(|p| &line[p.len()..])
        .unwrap_or(line)
}

pub fn apply_block_style(
    source: &str,
    sel: ByteSelection,
    style: BlockStyle,
) -> (String, ByteSelection) {
    let source = normalize_newlines(source);
    let spans = line_spans(&source);
    if spans.is_empty() {
        return (source, sel);
    }
    let (mut first, last) = selected_line_range(&source, sel);
    // First-line title formatting is implicit: skip the title line.
    if first == 0 {
        first = 1;
    }
    if first > last || first >= spans.len() {
        let len = source.len();
        return (source, sel.clamp(len));
    }
    let mut out = String::with_capacity(source.len() + 16);
    let mut new_anchor = sel.anchor;
    let mut new_cursor = sel.cursor;
    let mut cursor = 0;
    for (i, (ls, le)) in spans.iter().enumerate() {
        out.push_str(&source[cursor..*ls]);
        let line = &source[*ls..*le];
        if i >= first && i <= last {
            let rest = strip_block_prefix(line);
            let checked = checklist_prefix(line) == Some("- [x] ");
            let replacement = match style {
                BlockStyle::Title => format!("# {rest}"),
                BlockStyle::Heading => format!("## {rest}"),
                BlockStyle::Body => rest.to_string(),
                BlockStyle::Bullets => format!("- {rest}"),
                BlockStyle::Checklist => {
                    if checked {
                        format!("- [x] {rest}")
                    } else {
                        format!("- [ ] {rest}")
                    }
                }
                BlockStyle::Numbers => {
                    let n = i - first + 1;
                    format!("{n}. {rest}")
                }
            };
            let delta = replacement.len() as isize - line.len() as isize;
            out.push_str(&replacement);
            if sel.anchor >= *le {
                new_anchor = (new_anchor as isize + delta) as usize;
            } else if sel.anchor > *ls {
                new_anchor = (*ls + replacement.len()).min(out.len());
            }
            if sel.cursor >= *le {
                new_cursor = (new_cursor as isize + delta) as usize;
            } else if sel.cursor > *ls {
                new_cursor = (*ls + replacement.len()).min(out.len());
            }
        } else {
            out.push_str(line);
        }
        cursor = *le;
    }
    out.push_str(&source[cursor..]);
    let len = out.len();
    (
        out,
        ByteSelection {
            anchor: new_anchor.min(len),
            cursor: new_cursor.min(len),
        },
    )
}

pub fn apply_inline_style(
    source: &str,
    sel: ByteSelection,
    style: InlineStyle,
) -> (String, ByteSelection) {
    let source = normalize_newlines(source);
    let sel = sel.clamp(source.len());
    if sel.is_empty() {
        let insert = match style {
            InlineStyle::Bold => "****",
            InlineStyle::Italic => "**",
        };
        let (a, b) = source.split_at(sel.cursor);
        let mut out = String::with_capacity(source.len() + insert.len());
        out.push_str(a);
        out.push_str(insert);
        out.push_str(b);
        let inner = match style {
            InlineStyle::Bold => sel.cursor + 2,
            InlineStyle::Italic => sel.cursor + 1,
        };
        return (out, ByteSelection::caret(inner));
    }
    let projection = crate::projection::Projection::new(&source);
    let display = projection.display_selection(sel);
    let (out, selected) = projection.format_selection(&source, display, style);
    let next = crate::projection::Projection::new(&out);
    let display = next.display_selection(selected);
    let start = next.source_position(display.start(), false);
    let end = next.source_position(display.end(), true);
    let selection = if sel.anchor <= sel.cursor {
        ByteSelection { anchor: start, cursor: end }
    } else {
        ByteSelection { anchor: end, cursor: start }
    };
    (out, selection)
}

pub fn toggle_checkbox(
    source: &str,
    line_start: usize,
    note_id: NoteId,
    expected_id: NoteId,
    revision: u64,
    expected_revision: u64,
) -> Option<String> {
    if note_id != expected_id || revision != expected_revision {
        return None;
    }
    if line_start > source.len() || !source.is_char_boundary(line_start) {
        return None;
    }
    let rest = &source[line_start..];
    let line_end = rest
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(source.len());
    let line = &source[line_start..line_end];
    let replaced = if let Some(rest) = line.strip_prefix("- [ ] ") {
        format!("- [x] {rest}")
    } else if let Some(rest) = line.strip_prefix("- [x] ") {
        format!("- [ ] {rest}")
    } else {
        return None;
    };
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..line_start]);
    out.push_str(&replaced);
    out.push_str(&source[line_end..]);
    Some(out)
}

fn escape_unsupported(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '<' | '>' | '[' | ']' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.replace("```runsplash", "`\\`\\`runsplash")
}

pub fn preview_blocks(source: &str) -> Vec<PreviewBlock> {
    let source = normalize_newlines(source);
    let title = display_title(&source);
    let mut blocks = vec![PreviewBlock::Title(title)];
    let body = body_after_title(&source);
    let body_start = source.len() - body.len();
    let mut md = String::new();
    let mut fence: Option<usize> = None;
    let mut offset = body_start;
    // Skip a leading blank line after the title (the conventional `\n\n`).
    let mut lines = body.split_inclusive('\n').peekable();
    if matches!(lines.peek(), Some(l) if l.trim().is_empty()) {
        if let Some(l) = lines.next() {
            offset += l.len();
        }
    }
    let flush_md = |md: &mut String, blocks: &mut Vec<PreviewBlock>| {
        if !md.is_empty() {
            blocks.push(PreviewBlock::Markdown(std::mem::take(md)));
        }
    };
    let rest = &source[offset..];
    let mut idx = offset;
    for line in rest.split_inclusive('\n') {
        let content = line.trim_end_matches('\n');
        if let Some(_start) = fence {
            if content.trim_start().starts_with("```") {
                fence = None;
                md.push_str("```\n");
            } else {
                md.push_str(&escape_unsupported(line));
            }
            idx += line.len();
            continue;
        }
        if content.trim_start().starts_with("```runsplash") {
            flush_md(&mut md, &mut blocks);
            md.push_str("```\n");
            fence = Some(idx);
            idx += line.len();
            continue;
        }
        if content.trim_start().starts_with("```") {
            md.push_str(line);
            fence = Some(idx);
            idx += line.len();
            continue;
        }
        if let Some(label) = content.strip_prefix("- [ ] ") {
            flush_md(&mut md, &mut blocks);
            blocks.push(PreviewBlock::Checklist {
                line_start: idx,
                checked: false,
                label_markdown: label.to_string(),
            });
        } else if let Some(label) = content.strip_prefix("- [x] ") {
            flush_md(&mut md, &mut blocks);
            blocks.push(PreviewBlock::Checklist {
                line_start: idx,
                checked: true,
                label_markdown: label.to_string(),
            });
        } else {
            md.push_str(line);
        }
        idx += line.len();
    }
    flush_md(&mut md, &mut blocks);
    blocks
}

pub fn search_active<'a>(doc: &'a NotesDocument, query: &str) -> Vec<&'a Note> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let mut notes: Vec<&Note> = doc
        .notes
        .iter()
        .filter(|n| !n.is_deleted() && matches_query(n, q))
        .collect();
    notes.sort_by(|a, b| sort_active(a, b));
    notes
}

pub fn cap_preview_chars(s: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i >= max {
            break;
        }
        out.push(ch);
    }
    out
}

pub fn initial_ui(doc: &NotesDocument) -> NotesUi {
    let selected = first_result_id(doc, Collection::All, "");
    let mut ui = NotesUi {
        collection: Collection::All,
        selected,
        query: String::new(),
        route: vec![Screen::Folders],
        editor: selected.map(EditSession::new),
    };
    if selected.is_some() {
        // Wide starts with the first pinned note selected; compact still
        // begins on Folders. The selected note is ready either way.
        open_selected_read(doc, &mut ui);
    }
    ui
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed;

    fn seeded() -> NotesDocument {
        seed::generate(1_788_955_200_000)
    }

    #[test]
    fn escaped_unicode_is_safe_in_previews_and_search() {
        for text in ["é", "€", "漢字", "👨‍👩‍👧‍👦", "é"] {
            let source = format!("T\n\\{text}");
            assert_eq!(display_preview(&source), text);
            assert_eq!(plain_body(&source), text);
            let mut doc = NotesDocument::empty(0);
            let mut ui = NotesUi::default();
            let id = create_note(&mut doc, &mut ui, 0).unwrap();
            doc.note_mut(id).unwrap().source = source;
            assert_eq!(search_active(&doc, text).len(), 1);
            assert_eq!(crate::storage::decode(&crate::storage::encode(&doc).unwrap()).unwrap(), doc);
        }
    }

    #[test]
    fn deleting_after_clock_rollback_round_trips_before_restore() {
        let mut doc = crate::seed::generate(1_788_955_200_000);
        let mut ui = initial_ui(&doc);
        let id = NoteId(1);
        let before = doc.note(id).unwrap().clone();
        delete_note(&mut doc, &mut ui, id, 99).unwrap();
        assert_eq!(doc.note(id).unwrap().deleted_ms, Some(before.created_ms));
        assert_eq!(doc.note(id).unwrap().edited_ms, before.edited_ms);
        let persisted = crate::storage::decode(&crate::storage::encode(&doc).unwrap()).unwrap();
        assert_eq!(persisted, doc);
    }

    #[test]
    fn inline_formatting_preserves_checklist_prefixes_and_direction() {
        let source = "T\n- [ ] Café\n- [x] 漢字";
        let selection = ByteSelection { anchor: source.len(), cursor: source.find("Café").unwrap() };
        let (out, selected) = apply_inline_style(source, selection, InlineStyle::Bold);
        assert_eq!(out, "T\n- [ ] **Café**\n- [x] **漢字**");
        assert!(selected.anchor > selected.cursor);
        let projection = crate::projection::Projection::new(&out);
        assert_eq!(projection.paragraphs[1].style, crate::projection::ParagraphStyle::Check(false));
        assert_eq!(projection.paragraphs[2].style, crate::projection::ParagraphStyle::Check(true));
        assert_eq!(apply_inline_style(&out, selected, InlineStyle::Bold).0, source);
    }

    #[test]
    fn title_preview_crlf_and_unicode() {
        assert_eq!(display_title("Hello\nbody"), "Hello");
        assert_eq!(display_title("# Titled\n"), "Titled");
        assert_eq!(display_title("## Head\n"), "Head");
        assert_eq!(display_title("\nbody line"), "New Note");
        assert_eq!(display_title("   \nnext"), "New Note");
        assert_eq!(display_title("Café ☕\n"), "Café ☕");
        let crlf = normalize_newlines("Title\r\n\r\nBody\rline");
        assert_eq!(crlf, "Title\n\nBody\nline");
        assert_eq!(display_preview("Title\n\n"), "No additional text");
        assert_eq!(display_preview("Title\n\n- [ ] Milk"), "Milk");
        assert_eq!(
            display_preview("Title\n\n1. Zest one lemon."),
            "Zest one lemon."
        );
        assert_eq!(
            display_preview("Title\n\n**Check dates before booking.**"),
            "Check dates before booking."
        );
        assert_eq!(display_preview("Title\n\n*italic* line"), "italic line");
        assert_eq!(display_preview("Title\n\n# Shelving"), "Shelving");
    }

    #[test]
    fn sorting_pins_once_and_excludes_deleted() {
        let doc = seeded();
        let rows = list_rows(&doc, Collection::All, "", doc.seed_anchor_ms);
        let mut seen = std::collections::HashSet::new();
        let mut in_pinned = false;
        let mut pinned_ids = Vec::new();
        for row in &rows {
            match row {
                ListRow::Section(DateSection::Pinned) => in_pinned = true,
                ListRow::Section(_) => in_pinned = false,
                ListRow::Note(h) => {
                    assert!(seen.insert(h.id), "pin must appear once");
                    if in_pinned {
                        pinned_ids.push(h.id);
                        assert!(h.pinned);
                    } else {
                        assert!(!h.pinned, "pinned notes stay in the pinned section");
                    }
                    assert!(doc.note(h.id).map(|n| !n.is_deleted()).unwrap_or(false));
                }
            }
        }
        assert_eq!(pinned_ids, vec![NoteId(7), NoteId(1)]);
        let hits = list_hits(&doc, Collection::All, "agenda");
        assert!(hits.iter().all(|n| !n.is_deleted()));
        let work = list_hits(&doc, Collection::Folder(FolderId::Work), "");
        let mut prev = i64::MAX;
        for n in &work {
            assert!(n.edited_ms <= prev);
            prev = n.edited_ms;
        }
        let deleted = list_hits(&doc, Collection::RecentlyDeleted, "");
        assert_eq!(deleted.len(), 2);
        assert!(deleted[0].deleted_ms >= deleted[1].deleted_ms);
    }

    #[test]
    fn utc_date_groups_midnight_year_and_leap_day() {
        // 2024-03-01 00:00 UTC
        let now = 1_709_251_200_000;
        assert_eq!(date_section_for(now, now), DateSection::Today);
        assert_eq!(format_note_date(now, now), "00:00");
        assert_eq!(date_section_for(now - DAY_MS, now), DateSection::Yesterday);
        assert_eq!(format_note_date(now - DAY_MS, now), "Yesterday");
        assert_eq!(
            date_section_for(now - 2 * DAY_MS, now),
            DateSection::Previous7Days
        );
        assert_eq!(
            date_section_for(now - 7 * DAY_MS, now),
            DateSection::Previous7Days
        );
        assert_eq!(date_section_for(now - 8 * DAY_MS, now), DateSection::Older);
        // Leap day 2024-02-29 against 2024-03-01 is age 1 → Yesterday.
        let leap = civil::from_ymd(2024, 2, 29) as i64 * DAY_MS;
        assert_eq!(utc_day(leap), civil::from_ymd(2024, 2, 29));
        assert_eq!(date_section_for(leap, now), DateSection::Yesterday);
        // Year change: 2023-12-31 vs 2024-01-02
        let jan2 = civil::from_ymd(2024, 1, 2) as i64 * DAY_MS;
        let dec31 = civil::from_ymd(2023, 12, 31) as i64 * DAY_MS;
        assert_eq!(format_note_date(dec31, jan2), "31 Dec 2023");
        let jan1 = civil::from_ymd(2024, 1, 1) as i64 * DAY_MS;
        assert_eq!(format_note_date(jan1 + 15 * 60_000, jan2), "Yesterday");
        assert_eq!(
            format_note_date(jan2 + 3_600_000, jan2 + 4_000_000),
            "01:00"
        );
        let mar4 = civil::from_ymd(2024, 3, 4) as i64 * DAY_MS;
        assert_eq!(format_note_date(mar4, now + 10 * DAY_MS), "4 Mar");
    }

    #[test]
    fn search_case_unicode_and_order() {
        let doc = seeded();
        let lower = search_active(&doc, "lemon");
        let upper = search_active(&doc, "LEMON");
        assert_eq!(lower.len(), upper.len());
        assert_eq!(lower[0].id, NoteId(2));
        let uni = search_active(&doc, "café");
        assert!(uni.iter().any(|n| n.id == NoteId(9)));
        let mut prev = i64::MAX;
        for n in search_active(&doc, "the") {
            assert!(n.edited_ms <= prev);
            prev = n.edited_ms;
            assert!(!n.is_deleted());
        }
        assert!(search_active(&doc, "   ").is_empty());
    }

    #[test]
    fn create_limits_and_empty_persist() {
        let mut doc = seeded();
        let mut ui = initial_ui(&doc);
        let now = doc.seed_anchor_ms;
        let id = create_note(&mut doc, &mut ui, now).unwrap();
        assert_eq!(id, NoteId(21));
        assert_eq!(doc.next_id, 22);
        let note = doc.note(id).unwrap();
        assert_eq!(note.source, "");
        assert_eq!(display_title(&note.source), "New Note");
        assert_eq!(note.folder, FolderId::Notes);
        assert!(ui.query.is_empty());
        ui.collection = Collection::Folder(FolderId::Work);
        let id2 = create_note(&mut doc, &mut ui, now).unwrap();
        assert_eq!(doc.note(id2).unwrap().folder, FolderId::Work);
        doc.notes.truncate(MAX_NOTES);
        while doc.notes.len() < MAX_NOTES {
            doc.notes.push(Note {
                id: NoteId(doc.next_id),
                folder: FolderId::Notes,
                source: String::new(),
                pinned: false,
                created_ms: 0,
                edited_ms: 0,
                deleted_ms: None,
            });
            doc.next_id += 1;
        }
        assert!(create_note(&mut doc, &mut ui, 0).is_err());
        let mut session = EditSession::new(NoteId(1));
        let huge = "x".repeat(MAX_SOURCE_BYTES + 1);
        assert!(set_source(
            &mut doc,
            &mut session,
            huge,
            ByteSelection::caret(0),
            0,
            EditKind::Paste
        )
        .is_err());
        assert_eq!(
            doc.note(NoteId(1)).unwrap().source.lines().next().unwrap(),
            "Weekend groceries"
        );
    }

    #[test]
    fn pin_move_delete_restore() {
        // This successful deletion uses a clock after creation; rollback has
        // its own persistence regression above.
        let mut doc = crate::seed::generate(0);
        let mut ui = initial_ui(&doc);
        let src = doc.note(NoteId(2)).unwrap().source.clone();
        let edited = doc.note(NoteId(2)).unwrap().edited_ms;
        assert!(pin_toggle(&mut doc, NoteId(2)).unwrap());
        assert_eq!(doc.note(NoteId(2)).unwrap().edited_ms, edited);
        assert!(!pin_toggle(&mut doc, NoteId(2)).unwrap());
        move_note(&mut doc, NoteId(2), FolderId::Work).unwrap();
        assert_eq!(doc.note(NoteId(2)).unwrap().folder, FolderId::Work);
        assert_eq!(doc.note(NoteId(2)).unwrap().source, src);
        delete_note(&mut doc, &mut ui, NoteId(2), 99).unwrap();
        let n = doc.note(NoteId(2)).unwrap();
        assert_eq!(n.deleted_ms, Some(99));
        assert!(!n.pinned);
        assert_eq!(n.source, src);
        assert_eq!(n.folder, FolderId::Work);
        restore_note(&mut doc, &mut ui, NoteId(2)).unwrap();
        let n = doc.note(NoteId(2)).unwrap();
        assert!(n.deleted_ms.is_none());
        assert_eq!(n.edited_ms, edited);
        assert_eq!(
            folder_counts(&doc)
                .iter()
                .find(|c| c.collection == Collection::RecentlyDeleted)
                .unwrap()
                .count,
            2
        );
    }

    #[test]
    fn formatting_lines_and_numbers() {
        let src = "Title\n\nhello\nworld\n";
        let (out, _) = apply_block_style(
            src,
            ByteSelection {
                anchor: 7,
                cursor: 7,
            },
            BlockStyle::Title,
        );
        assert!(out.contains("# hello"));
        assert!(out.starts_with("Title\n"));
        let (out, _) = apply_block_style(
            &out,
            ByteSelection {
                anchor: 7,
                cursor: 7,
            },
            BlockStyle::Title,
        );
        assert_eq!(out.matches("# hello").count(), 1);
        let (out, _) = apply_block_style(
            src,
            ByteSelection {
                anchor: 7,
                cursor: 18,
            },
            BlockStyle::Heading,
        );
        assert!(out.contains("## hello"));
        assert!(out.contains("## world"));
        let (out, _) = apply_block_style(
            &out,
            ByteSelection {
                anchor: 7,
                cursor: 30,
            },
            BlockStyle::Body,
        );
        assert!(!out.contains("## "));
        let (out, _) = apply_block_style(
            src,
            ByteSelection {
                anchor: 7,
                cursor: 18,
            },
            BlockStyle::Bullets,
        );
        assert!(out.contains("- hello"));
        let (out, _) = apply_block_style(
            src,
            ByteSelection {
                anchor: 7,
                cursor: 18,
            },
            BlockStyle::Numbers,
        );
        assert!(out.contains("1. hello"));
        assert!(out.contains("2. world"));
        let checked = "Title\n\n- [x] done\n";
        let (out, _) = apply_block_style(checked, ByteSelection::caret(8), BlockStyle::Checklist);
        assert!(out.contains("- [x] done"));
        // Reversed selection, ending exactly at next line start, excludes next line.
        let src = "Title\n\nhello\nworld";
        let hello = src.find("hello").unwrap();
        let world = src.find("world").unwrap();
        let (out, _) = apply_block_style(
            src,
            ByteSelection {
                anchor: world,
                cursor: hello,
            },
            BlockStyle::Bullets,
        );
        assert!(out.contains("- hello"));
        assert!(!out.contains("- world"));
    }

    #[test]
    fn bold_italic_wrap_and_unicode() {
        let src = "Title\n\nhello";
        let hello = src.find("hello").unwrap();
        let (out, sel) = apply_inline_style(
            src,
            ByteSelection {
                anchor: hello,
                cursor: hello + 5,
            },
            InlineStyle::Bold,
        );
        assert!(out.contains("**hello**"));
        assert_eq!(&out[sel.start()..sel.end()], "**hello**");
        let (out2, _) = apply_inline_style(&out, sel, InlineStyle::Bold);
        assert!(out2.contains("hello"));
        assert!(!out2.contains("**hello**"));
        let (out, sel) = apply_inline_style(src, ByteSelection::caret(hello), InlineStyle::Bold);
        assert_eq!(&out[sel.cursor - 2..sel.cursor + 2], "****");
        let cafe = "Title\n\nCafé";
        let at = cafe.find("Café").unwrap();
        let (out, sel) = apply_inline_style(
            cafe,
            ByteSelection {
                anchor: at,
                cursor: cafe.len(),
            },
            InlineStyle::Italic,
        );
        assert!(out.contains("*Café*"));
        assert!(out.is_char_boundary(sel.start()) && out.is_char_boundary(sel.end()));
        let multi = "Title\n\nabc\ndef";
        let a = multi.find("abc").unwrap();
        let (out, _) = apply_inline_style(
            multi,
            ByteSelection {
                anchor: a,
                cursor: multi.len(),
            },
            InlineStyle::Bold,
        );
        assert!(out.contains("**abc**"));
        assert!(out.contains("**def**"));
    }

    #[test]
    fn checkbox_toggle_and_stale() {
        let src = "Title\n\n- [ ] Milk\n- [x] Eggs";
        let line = src.find("- [ ]").unwrap();
        let once = toggle_checkbox(src, line, NoteId(1), NoteId(1), 3, 3).unwrap();
        assert!(once.contains("- [x] Milk"));
        assert!(once.contains("- [x] Eggs"));
        let twice = toggle_checkbox(&once, line, NoteId(1), NoteId(1), 4, 4).unwrap();
        assert_eq!(twice, src);
        assert!(toggle_checkbox(src, line, NoteId(1), NoteId(2), 3, 3).is_none());
        assert!(toggle_checkbox(src, line, NoteId(1), NoteId(1), 3, 9).is_none());
        assert!(toggle_checkbox(src, 0, NoteId(1), NoteId(1), 3, 3).is_none());
    }

    #[test]
    fn preview_blocks_order_and_runsplash() {
        let src = "Weekend groceries\n\nSaturday market.\n- [ ] Tomatoes\n- [x] Coffee\nMore text\n```runsplash\ncrash()\n```\n";
        let blocks = preview_blocks(src);
        assert!(matches!(&blocks[0], PreviewBlock::Title(t) if t == "Weekend groceries"));
        assert!(blocks
            .iter()
            .any(|b| matches!(b, PreviewBlock::Markdown(m) if m.contains("Saturday"))));
        let checks: Vec<_> = blocks
            .iter()
            .filter_map(|b| match b {
                PreviewBlock::Checklist {
                    checked,
                    label_markdown,
                    ..
                } => Some((*checked, label_markdown.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(checks, vec![(false, "Tomatoes"), (true, "Coffee")]);
        let joined = blocks
            .iter()
            .filter_map(|b| match b {
                PreviewBlock::Markdown(m) => Some(m.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        assert!(!joined.contains("```runsplash"));
        assert!(joined.contains("crash()") || joined.contains("```"));
    }

    #[test]
    fn undo_groups_and_bounds() {
        let mut doc = seeded();
        let mut session = EditSession::new(NoteId(1));
        session.mode = EditorMode::Edit;
        let base = doc.note(NoteId(1)).unwrap().source.clone();
        set_source(
            &mut doc,
            &mut session,
            format!("{base}a"),
            ByteSelection::caret(base.len() + 1),
            1000,
            EditKind::Typing,
        )
        .unwrap();
        set_source(
            &mut doc,
            &mut session,
            format!("{base}ab"),
            ByteSelection::caret(base.len() + 2),
            1100,
            EditKind::Typing,
        )
        .unwrap();
        assert_eq!(session.undo.len(), 1, "typing coalesces within 500ms");
        set_source(
            &mut doc,
            &mut session,
            format!("{base}ab**x**"),
            ByteSelection::caret(base.len() + 3),
            2000,
            EditKind::Format,
        )
        .unwrap();
        assert_eq!(session.undo.len(), 2);
        let formatted = doc.note(NoteId(1)).unwrap().source.clone();
        undo(&mut doc, &mut session, 3000);
        assert_ne!(doc.note(NoteId(1)).unwrap().source, formatted);
        redo(&mut doc, &mut session, 4000);
        assert_eq!(doc.note(NoteId(1)).unwrap().source, formatted);
        set_source(
            &mut doc,
            &mut session,
            format!("{formatted}!"),
            ByteSelection::caret(formatted.len() + 1),
            5000,
            EditKind::Paste,
        )
        .unwrap();
        assert!(session.redo.is_empty());
        for i in 0..40 {
            let src = format!("{}{i}", doc.note(NoteId(1)).unwrap().source);
            let len = src.len();
            set_source(
                &mut doc,
                &mut session,
                src,
                ByteSelection::caret(len),
                10_000 + i * 1000,
                EditKind::Paste,
            )
            .unwrap();
        }
        assert!(session.undo.len() <= MAX_UNDO);
        session.mode = EditorMode::Read;
        assert!(!session.undo.is_empty(), "history survives Read/Edit");
    }

    #[test]
    fn layout_breakpoints() {
        assert_eq!(decide_layout(402.0, 780.0, None).family, Family::Compact);
        assert_eq!(decide_layout(699.0, 800.0, None).family, Family::Compact);
        let w = decide_layout(700.0, 800.0, None);
        assert_eq!(w.family, Family::Wide);
        assert_eq!(w.columns, Columns::Two);
        let three = decide_layout(1240.0, 800.0, None);
        assert_eq!(three.columns, Columns::Three);
        assert!(!three.short_chrome);
        let land = decide_layout(874.0, 300.0, None);
        assert_eq!(land.family, Family::Wide);
        assert_eq!(land.columns, Columns::Two);
        assert!(land.short_chrome);
        assert_eq!(list_column_width(land, 874.0), 280.0);
        let compact = decide_layout(402.0, 780.0, None);
        assert_eq!(decide_layout(0.0, 0.0, Some(compact)), compact);
        assert_eq!(decide_layout(1.0, 800.0, Some(three)), three);
        assert_eq!(decide_layout(0.5, 0.5, None), LayoutDecision::DEFAULT);
    }

    #[test]
    fn route_back_and_resize_preserve() {
        let mut doc = seeded();
        let mut ui = initial_ui(&doc);
        assert_eq!(current_screen(&ui), Screen::Folders);
        select_collection(&doc, &mut ui, Collection::Folder(FolderId::Work));
        assert_eq!(current_screen(&ui), Screen::List);
        let selected = ui.selected.unwrap();
        select_note(&doc, &mut ui, selected);
        assert_eq!(current_screen(&ui), Screen::Editor);
        let query = "review";
        set_query(&doc, &mut ui, query.into());
        let source = doc.note(selected).unwrap().source.clone();
        let sel = ui.selected;
        go_back(&mut ui);
        assert_eq!(current_screen(&ui), Screen::List);
        go_back(&mut ui);
        assert_eq!(current_screen(&ui), Screen::Folders);
        go_back(&mut ui);
        go_back(&mut ui);
        assert_eq!(current_screen(&ui), Screen::Folders);
        assert_eq!(ui.selected, sel);
        assert_eq!(ui.query, query);
        assert_eq!(doc.note(selected).unwrap().source, source);
        delete_note(&mut doc, &mut ui, selected, 1).unwrap();
        assert_ne!(ui.selected, Some(selected));
    }
    #[test]
    fn initial_selection_and_search_session_follow_rendered_groups() {
        let mut doc = seeded();
        doc.note_mut(NoteId(2)).unwrap().edited_ms = doc.seed_anchor_ms + 1000;
        let mut ui = initial_ui(&doc);
        let first = list_rows(&doc, Collection::All, "", doc.seed_anchor_ms)
            .into_iter()
            .find_map(|row| match row {
                ListRow::Note(hit) => Some(hit.id),
                _ => None,
            });
        assert_eq!(ui.selected, first);
        set_query(&doc, &mut ui, "lemon".into());
        assert_eq!(ui.selected, Some(NoteId(2)));
        assert_eq!(ui.editor.as_ref().unwrap().note_id, NoteId(2));
        select_collection(&doc, &mut ui, Collection::Folder(FolderId::Notes));
        move_note(&mut doc, NoteId(2), FolderId::Work).unwrap();
        normalize_after_filter(&doc, &mut ui);
        assert!(ui.selected.is_none());
        assert!(ui.editor.is_none());
    }
}
