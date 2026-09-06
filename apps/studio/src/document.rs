//! UI-owned shared documents and a real CodeEditor view. Filesystem reads live
//! in document_worker; clean disk updates preserve sessions, dirty ones conflict.

use crate::document_worker::{FileSnapshot, MAX_DOCUMENTS, MAX_FILE_BYTES};
use makepad_code_editor::{
    code_editor::{CodeEditorAction, KeepCursorInView},
    decoration::DecorationSet,
    history::EditKind,
    selection::SelectionSet,
    text::{Change, Drift, Edit, Position},
    CodeDocument, CodeEditor, CodeSession,
};
use makepad_widgets::*;
use std::{
    cell::RefCell,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
    sync::Arc,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*
    use mod.math.*
    mod.widgets.StudioCodeEditorBase = #(StudioCodeEditor::register_widget(vm))
    mod.widgets.StudioCodeEditor = set_type_default() do mod.widgets.StudioCodeEditorBase {
        width: Fill height: Fill
        editor +: {
            width: Fill height: Fill
            pad_left_top: vec2(12.0, 10.0)
            empty_page_at_end: false
            read_only: false
            show_gutter: true
            word_wrap: false
            scroll_bars: mod.widgets.ScrollBars {}
            draw_bg +: { color: theme.color_bg_app }
            draw_gutter +: {
                color: mix(theme.color_bg_app, theme.color_text, 0.52)
                text_style: theme.font_code
            }
            draw_text +: { text_style: theme.font_code get_brightness: fn(){return 1.0} }
            draw_cursor +: {color: theme.color_text}
            token_colors +: {
                whitespace: theme.color_text_meta
                delimiter: theme.color_text
                delimiter_highlight: theme.color_text
                unknown: theme.color_text
                identifier: theme.color_text
                punctuator: theme.color_text
                branch_keyword: mix(theme.color_text, #b87bc3, 0.45)
                loop_keyword: mix(theme.color_text, #d99447, 0.45)
                other_keyword: mix(theme.color_text, #4e95cc, 0.6)
                constant: mix(theme.color_text, #c97554, 0.45)
                number: mix(theme.color_text, #67a85b, 0.45)
                string: mix(theme.color_text, #c97554, 0.45)
                function: mix(theme.color_text, #a27c26, 0.35)
                typename: mix(theme.color_text, #4bab99, 0.45)
                comment: mix(theme.color_text, #729467, 0.5)
                error_decoration: theme.color_error
                warning_decoration: theme.color_text
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedRange {
    pub start: Position,
    pub end: Position,
    pub removed_bytes: usize,
    pub inserted_bytes: usize,
}

#[derive(Clone, Debug)]
struct Metadata {
    baseline: Arc<String>,
    disk: Arc<String>,
    observed_revision: u64,
    saved_revision: u64,
    revision: u64,
    dirty: bool,
    conflict: bool,
    error: Option<String>,
    last_change: Option<ChangedRange>,
}

struct DocumentInner {
    path: PathBuf,
    document: CodeDocument,
    external_session: RefCell<CodeSession>,
    /// UI-only weak handles let a shared update drain inactive views too.
    /// CodeSession's internal notifications cannot grow while a tab is hidden.
    sessions: RefCell<Vec<Weak<RefCell<CodeSession>>>>,
    metadata: RefCell<Metadata>,
}

#[derive(Clone)]
pub struct DocumentHandle(Rc<DocumentInner>);

impl DocumentHandle {
    fn new(path: PathBuf, text: Arc<String>, observed_revision: u64) -> Self {
        let document = CodeDocument::new(text.as_str().into(), DecorationSet::new());
        let external_session = RefCell::new(CodeSession::new(document.clone()));
        Self(Rc::new(DocumentInner {
            path,
            document,
            external_session,
            sessions: RefCell::new(Vec::new()),
            metadata: RefCell::new(Metadata {
                baseline: text.clone(),
                disk: text,
                observed_revision,
                saved_revision: 0,
                revision: 1,
                dirty: false,
                conflict: false,
                error: None,
                last_change: None,
            }),
        }))
    }

    pub fn path(&self) -> &Path {
        &self.0.path
    }
    pub fn current_text(&self) -> String {
        self.0.document.as_text().to_string()
    }
    pub fn revision(&self) -> u64 {
        self.0.metadata.borrow().revision
    }
    pub fn disk_revision(&self) -> u64 {
        self.0.metadata.borrow().observed_revision
    }
    pub fn is_dirty(&self) -> bool {
        self.0.metadata.borrow().dirty
    }
    pub fn has_conflict(&self) -> bool {
        self.0.metadata.borrow().conflict
    }
    pub fn disk_text(&self) -> Arc<String> {
        self.0.metadata.borrow().disk.clone()
    }
    pub fn last_change(&self) -> Option<ChangedRange> {
        self.0.metadata.borrow().last_change.clone()
    }
    pub fn same_document(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub fn status(&self) -> String {
        let meta = self.0.metadata.borrow();
        if let Some(error) = &meta.error {
            return format!("File unavailable: {error}; open buffer retained");
        }
        if meta.conflict {
            return "Conflict: disk changed while this buffer has local edits; local text retained"
                .into();
        }
        if meta.dirty {
            return "Unsaved local edits · disk monitoring continues".into();
        }
        match &meta.last_change {
            Some(change) => format!(
                "Live disk update · lines {}–{} · author unknown",
                change.start.line_index + 1,
                change.end.line_index + 1
            ),
            None => "Watching disk · local edits remain in this buffer until saved".into(),
        }
    }

    fn view_session(&self) -> Rc<RefCell<CodeSession>> {
        let session = Rc::new(RefCell::new(CodeSession::new(self.0.document.clone())));
        let mut sessions = self.0.sessions.borrow_mut();
        sessions.retain(|s| s.strong_count() > 0);
        sessions.push(Rc::downgrade(&session));
        session
    }

    fn drain_sessions(&self) {
        self.0.external_session.borrow_mut().handle_changes();
        self.0.sessions.borrow_mut().retain(|weak| {
            if let Some(session) = weak.upgrade() {
                session.borrow_mut().handle_changes();
                true
            } else {
                false
            }
        });
    }

    fn apply_text(&self, old: &str, new: &str) -> Option<ChangedRange> {
        let delta = minimal_delta(old, new)?;
        let start = position_at(old, delta.old.start);
        let end = position_at(old, delta.old.end);
        let new_end =
            start + makepad_code_editor::text::Text::from(delta.inserted.as_str()).length();
        let origin = self.0.external_session.borrow().id();
        self.0.document.force_new_group();
        // One default line selection invokes this callback exactly once.
        // edit_linewise publishes ordinary edits without auto-indenting disk text.
        self.0.document.edit_linewise(
            origin,
            EditKind::Other,
            &SelectionSet::default(),
            |mut editor, _| {
                if end != start {
                    editor.apply_edit(Edit {
                        change: Change::Delete(start, end - start),
                        drift: Drift::Before,
                    });
                }
                if !delta.inserted.is_empty() {
                    editor.apply_edit(Edit {
                        change: Change::Insert(start, delta.inserted.as_str().into()),
                        drift: Drift::Before,
                    });
                }
            },
        );
        self.0.document.force_new_group();
        self.drain_sessions();
        Some(ChangedRange {
            start,
            end: new_end,
            removed_bytes: delta.old.len(),
            inserted_bytes: delta.inserted.len(),
        })
    }

    /// Called after the editor emits TextDidChange. No disk state is changed.
    pub fn local_text_changed(&self) {
        self.drain_sessions();
        let text = self.current_text();
        let mut meta = self.0.metadata.borrow_mut();
        if text == *meta.disk {
            meta.baseline = meta.disk.clone();
            meta.dirty = false;
            meta.conflict = false;
        } else {
            meta.dirty = text != *meta.baseline;
            meta.conflict = meta.disk != meta.baseline;
        }
        meta.revision += 1;
        meta.last_change = None;
    }

    /// Accept a completed disk observation, never inventing a typing stream or author.
    pub fn external_update(&self, text: Arc<String>, observed_revision: u64) -> bool {
        let (baseline, seen) = {
            let meta = self.0.metadata.borrow();
            (meta.baseline.clone(), meta.observed_revision)
        };
        if observed_revision <= seen || text.len() > MAX_FILE_BYTES {
            return false;
        }
        let local = self.current_text();
        let clean = local == *baseline;
        let converged = local == *text;
        let change = if clean {
            self.apply_text(&local, &text)
        } else {
            None
        };
        let mut meta = self.0.metadata.borrow_mut();
        meta.observed_revision = observed_revision;
        meta.disk = text.clone();
        meta.error = None;
        if clean || converged {
            meta.baseline = text;
            meta.dirty = false;
            meta.conflict = false;
        } else {
            meta.dirty = true;
            meta.conflict = meta.disk != meta.baseline;
        }
        meta.last_change = change;
        meta.revision += 1;
        true
    }

    fn unavailable(&self, revision: u64, error: String) {
        let mut meta = self.0.metadata.borrow_mut();
        if revision <= meta.observed_revision {
            return;
        }
        meta.observed_revision = revision;
        meta.error = Some(error);
        meta.revision += 1;
    }

    /// Acknowledge a worker save. New edits made after submission remain dirty;
    /// a newer disk observation is retained even if channels were polled first.
    pub fn save_succeeded(&self, saved_text: Arc<String>, saved_revision: u64) {
        let mut local = self.current_text();
        let newer_disk = {
            let meta = self.0.metadata.borrow();
            if saved_revision <= meta.saved_revision {
                return;
            }
            if local == *saved_text
                && meta.observed_revision > saved_revision
                && meta.error.is_none()
            {
                Some(meta.disk.clone())
            } else {
                None
            }
        };
        // Disk and save acknowledgements have separate channels. If no human
        // edits followed this save, a later disk edit is safe to apply now.
        if let Some(disk) = newer_disk {
            self.apply_text(&local, &disk);
            local = disk.as_ref().clone();
        }
        let mut meta = self.0.metadata.borrow_mut();
        meta.saved_revision = saved_revision;
        if saved_revision >= meta.observed_revision {
            meta.disk = saved_text.clone();
            meta.observed_revision = saved_revision;
            meta.error = None;
        }
        meta.baseline = saved_text;
        if local == *meta.disk {
            meta.baseline = meta.disk.clone();
            meta.dirty = false;
            meta.conflict = false;
        } else {
            meta.dirty = local != *meta.baseline;
            meta.conflict = meta.disk != meta.baseline;
        }
        meta.last_change = None;
        meta.revision += 1;
    }

    /// Explicitly discard the local buffer in favor of the latest observed disk
    /// text. Root must expose this as an intentional user/tool action, not a timer.
    pub fn discard_local_and_reload(&self) -> Result<(), String> {
        let disk = {
            let meta = self.0.metadata.borrow();
            if let Some(error) = &meta.error {
                return Err(format!("Cannot reload unavailable disk state: {error}"));
            }
            meta.disk.clone()
        };
        let change = self.apply_text(&self.current_text(), &disk);
        let mut meta = self.0.metadata.borrow_mut();
        meta.baseline = disk;
        meta.dirty = false;
        meta.conflict = false;
        meta.last_change = change;
        meta.revision += 1;
        Ok(())
    }
}

#[derive(Default)]
pub struct DocumentRegistry {
    documents: HashMap<PathBuf, DocumentHandle>,
    aliases: HashMap<PathBuf, PathBuf>,
}

impl DocumentRegistry {
    pub fn get(&self, path: &Path) -> Option<DocumentHandle> {
        self.documents
            .get(self.aliases.get(path).map(PathBuf::as_path).unwrap_or(path))
            .cloned()
    }

    pub fn handles(&self) -> impl Iterator<Item = &DocumentHandle> {
        self.documents.values()
    }

    pub fn apply_snapshot(&mut self, snapshot: &FileSnapshot) -> Result<DocumentHandle, String> {
        let existing = self
            .get(&snapshot.path)
            .or_else(|| self.get(&snapshot.requested_path));
        if let Some(error) = &snapshot.error {
            if let Some(handle) = existing {
                handle.unavailable(snapshot.revision, error.clone());
                return Ok(handle);
            }
            return Err(format!("{}: {error}", snapshot.requested_path.display()));
        }
        let text = snapshot.text.clone().ok_or("File snapshot has no text")?;
        if text.len() > MAX_FILE_BYTES {
            return Err("Document exceeds the 2 MiB limit".into());
        }
        let handle = if let Some(handle) = existing {
            handle.external_update(text, snapshot.revision);
            handle
        } else {
            if self.documents.len() >= MAX_DOCUMENTS {
                return Err(format!("At most {MAX_DOCUMENTS} documents can be retained"));
            }
            let handle = DocumentHandle::new(snapshot.path.clone(), text, snapshot.revision);
            self.documents.insert(snapshot.path.clone(), handle.clone());
            handle
        };
        self.aliases
            .insert(snapshot.requested_path.clone(), handle.path().to_owned());
        Ok(handle)
    }

    /// A close must explicitly decide what to do with unsaved text first.
    pub fn remove(&mut self, path: &Path) -> Result<(), String> {
        let Some(handle) = self.get(path) else {
            return Ok(());
        };
        if handle.is_dirty() || handle.has_conflict() {
            return Err("Document has local edits or a disk conflict".into());
        }
        let canonical = handle.path().to_owned();
        self.documents.remove(&canonical);
        self.aliases.retain(|_, value| value != &canonical);
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub enum StudioCodeEditorAction {
    Changed(PathBuf),
    #[default]
    None,
}

#[derive(Script, ScriptHook, WidgetRef, WidgetSet, WidgetRegister)]
pub struct StudioCodeEditor {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[live]
    editor: CodeEditor,
    #[rust]
    document: Option<DocumentHandle>,
    #[rust]
    session: Option<Rc<RefCell<CodeSession>>>,
    #[rust]
    seen_revision: u64,
    #[rust]
    canvas_anchor: Option<(Area, PopupAnchorTransform)>,
}

impl StudioCodeEditor {
    pub fn bind_document(&mut self, cx: &mut Cx, document: DocumentHandle) {
        if self
            .document
            .as_ref()
            .is_some_and(|old| old.same_document(&document))
        {
            self.sync_document(cx);
            return;
        }
        self.session = Some(document.view_session());
        self.seen_revision = document.revision();
        self.document = Some(document);
        self.editor.keep_cursor_in_view = KeepCursorInView::Once;
        self.editor.set_scroll_pos(cx, dvec2(0.0, 0.0));
        self.editor.redraw(cx);
    }

    pub fn document(&self) -> Option<DocumentHandle> {
        self.document.clone()
    }
    pub fn current_text(&self) -> String {
        self.document
            .as_ref()
            .map(DocumentHandle::current_text)
            .unwrap_or_default()
    }
    pub fn status(&self) -> String {
        self.document
            .as_ref()
            .map(DocumentHandle::status)
            .unwrap_or_else(|| "Open a source file to follow its changes".into())
    }
    pub fn set_canvas_anchor(&mut self, anchor: Option<(Area, PopupAnchorTransform)>) {
        self.canvas_anchor = anchor;
    }

    /// Root calls this after registry updates; hidden views keep their sessions
    /// and need no frame loop. The underlying document drains all session edits.
    pub fn sync_document(&mut self, cx: &mut Cx) {
        let Some(document) = &self.document else {
            return;
        };
        if document.revision() != self.seen_revision {
            self.seen_revision = document.revision();
            if let Some(session) = &self.session {
                session.borrow_mut().handle_changes();
            }
            self.editor.keep_cursor_in_view = KeepCursorInView::Off;
            self.editor.redraw(cx);
        }
    }

    pub fn focus(&mut self, cx: &mut Cx) {
        self.editor.set_key_focus(cx);
    }
}

impl WidgetNode for StudioCodeEditor {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }
    fn walk(&mut self, _: &mut Cx) -> Walk {
        self.walk
    }
    fn area(&self) -> Area {
        self.editor.area()
    }
    fn redraw(&mut self, cx: &mut Cx) {
        self.editor.redraw(cx);
    }
    fn find_widgets_from_point(&self, cx: &Cx, point: DVec2, found: &mut dyn FnMut(&WidgetRef)) {
        self.editor.find_widgets_from_point(cx, point, found);
    }
    fn visible(&self) -> bool {
        self.editor.visible()
    }
    fn set_visible(&mut self, cx: &mut Cx, visible: bool) {
        self.editor.set_visible(cx, visible);
    }
}

impl Widget for StudioCodeEditor {
    fn draw_walk(&mut self, cx: &mut Cx2d, _: &mut Scope, walk: Walk) -> DrawStep {
        self.sync_document(cx);
        if let Some(session) = &self.session {
            self.editor
                .draw_walk_editor(cx, &mut session.borrow_mut(), walk);
            if let Some((canvas_area, transform)) = self.canvas_anchor {
                let (area, cursor) = self.editor.ime_anchor(cx);
                if !area.is_empty() && cx.has_key_focus(area) {
                    let screen = (area.clipped_rect(cx).pos + cursor) * transform.scale
                        + transform.translation;
                    let cursor = screen - canvas_area.rect(cx).pos;
                    cx.show_text_ime(canvas_area, cursor);
                }
            }
        } else {
            self.editor.draw_empty_editor(cx, walk);
        }
        DrawStep::done()
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.sync_document(cx);
        let Some(session) = &self.session else {
            return;
        };
        let changed = {
            let mut session = session.borrow_mut();
            self.editor
                .handle_event(cx, event, scope, &mut session)
                .into_iter()
                .any(|a| matches!(a, CodeEditorAction::TextDidChange))
        };
        if changed {
            if let Some(document) = &self.document {
                document.local_text_changed();
                self.seen_revision = document.revision();
                cx.widget_action(
                    self.uid,
                    StudioCodeEditorAction::Changed(document.path().to_owned()),
                );
            }
        }
    }

    fn text(&self) -> String {
        self.current_text()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Delta {
    old: Range<usize>,
    inserted: String,
}

/// A bounded linear prefix/suffix edit. Stable surrounding lines remain in
/// place; a caret inside the replaced span follows CodeSession's edit mapping.
fn minimal_delta(old: &str, new: &str) -> Option<Delta> {
    if old == new {
        return None;
    }
    let mut start = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(start) || !new.is_char_boundary(start) {
        start -= 1;
    }
    let mut suffix = old.as_bytes()[start..]
        .iter()
        .rev()
        .zip(new.as_bytes()[start..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    Some(Delta {
        old: start..old.len() - suffix,
        inserted: new[start..new.len() - suffix].to_owned(),
    })
}

fn position_at(text: &str, byte: usize) -> Position {
    let prefix = &text[..byte];
    Position {
        line_index: prefix.bytes().filter(|b| *b == b'\n').count(),
        byte_index: prefix.rfind('\n').map_or(byte, |i| byte - i - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_code_editor::{history::NewGroup, selection::Affinity, session::SelectionMode};

    fn document(text: &str) -> DocumentHandle {
        DocumentHandle::new(PathBuf::from("/work/source.rs"), Arc::new(text.into()), 1)
    }

    #[test]
    fn delta_keeps_unicode_boundaries_and_unchanged_tail() {
        let old = "start\nαéold\nunchanged\n";
        let new = "start\nαênew\nunchanged\n";
        let delta = minimal_delta(old, new).unwrap();
        let mut rebuilt = old.to_owned();
        rebuilt.replace_range(delta.old.clone(), &delta.inserted);
        assert_eq!(rebuilt, new);
        assert_eq!(&old[..delta.old.start], "start\nα");
        assert_eq!(&old[delta.old.end..], "\nunchanged\n");
        for (old, new) in [("", "é"), ("é", ""), ("α", "β"), ("abc", "ab🙂c")] {
            let delta = minimal_delta(old, new).unwrap();
            let mut s = old.to_owned();
            s.replace_range(delta.old, &delta.inserted);
            assert_eq!(s, new);
        }
    }

    #[test]
    fn external_edit_preserves_shared_sessions_and_unaffected_cursor() {
        let doc = document("first\nkeep cursor here\n");
        let a = doc.view_session();
        let b = doc.view_session();
        a.borrow().set_selection(
            Position {
                line_index: 1,
                byte_index: 4,
            },
            Affinity::Before,
            SelectionMode::Simple,
            NewGroup::Yes,
        );
        assert!(doc.external_update(Arc::new("new line\nfirst\nkeep cursor here\n".into()), 2));
        assert_eq!(
            doc.current_text(),
            b.borrow().document().as_text().to_string()
        );
        assert_eq!(
            a.borrow().selections()[0].cursor.position,
            Position {
                line_index: 2,
                byte_index: 4
            }
        );
        assert!(!doc.is_dirty());
        assert!(!doc.has_conflict());
        assert!(!doc.external_update(Arc::new("stale".into()), 1));
    }

    #[test]
    fn dirty_human_edits_are_not_overwritten_and_reload_is_explicit() {
        let doc = document("base\n");
        let view = doc.view_session();
        view.borrow().paste("human ".into());
        doc.local_text_changed();
        assert!(doc.is_dirty());
        let human = doc.current_text();
        doc.external_update(Arc::new("agent\n".into()), 2);
        assert_eq!(doc.current_text(), human);
        assert!(doc.has_conflict());
        assert_eq!(doc.disk_text().as_str(), "agent\n");
        doc.discard_local_and_reload().unwrap();
        assert_eq!(doc.current_text(), "agent\n");
        assert!(!doc.is_dirty());
        assert!(!doc.has_conflict());
    }

    #[test]
    fn save_ack_preserves_edits_made_while_save_was_pending() {
        let doc = document("base\n");
        let view = doc.view_session();
        view.borrow().paste("saved ".into());
        doc.local_text_changed();
        let saved = Arc::new(doc.current_text());
        view.borrow().paste("later ".into());
        doc.local_text_changed();
        let newer_local = doc.current_text();
        doc.save_succeeded(saved.clone(), 2);
        assert_eq!(doc.current_text(), newer_local);
        assert_eq!(doc.disk_text(), saved);
        assert!(doc.is_dirty());
        assert!(!doc.has_conflict());
        let latest = Arc::new(doc.current_text());
        doc.save_succeeded(latest.clone(), 3);
        assert!(!doc.is_dirty());
        doc.save_succeeded(Arc::new("stale acknowledgement".into()), 2);
        assert_eq!(doc.disk_text(), latest);
        assert!(!doc.is_dirty());
    }

    #[test]
    fn late_save_ack_reconciles_newer_disk_observation() {
        let doc = document("base\n");
        let view = doc.view_session();
        view.borrow().paste("human ".into());
        doc.local_text_changed();
        let saved = Arc::new(doc.current_text());
        doc.external_update(Arc::new("subsequent agent edit\n".into()), 3);
        assert!(doc.has_conflict());
        doc.save_succeeded(saved, 2);
        assert_eq!(doc.current_text(), "subsequent agent edit\n");
        assert!(!doc.is_dirty());
        assert!(!doc.has_conflict());
        assert_eq!(doc.disk_revision(), 3);
    }

    #[test]
    fn canonical_aliases_share_a_document_and_errors_retain_text() {
        let mut registry = DocumentRegistry::default();
        let mut s = FileSnapshot {
            requested_path: PathBuf::from("/alias/file"),
            path: PathBuf::from("/work/file"),
            revision: 1,
            text: Some(Arc::new("source".into())),
            error: None,
        };
        let a = registry.apply_snapshot(&s).unwrap();
        s.requested_path = PathBuf::from("/work/file");
        s.revision = 2;
        let b = registry.apply_snapshot(&s).unwrap();
        assert!(a.same_document(&b));
        s.revision = 3;
        s.text = None;
        s.error = Some("deleted".into());
        registry.apply_snapshot(&s).unwrap();
        assert_eq!(a.current_text(), "source");
        assert!(a.status().contains("deleted"));
    }
}
